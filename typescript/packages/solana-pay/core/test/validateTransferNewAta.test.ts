import type { Address, GetTransactionApi, Rpc } from '@solana/kit';
import {
    AccountRole,
    appendTransactionMessageInstructions,
    createClient,
    createTransactionMessage,
    generateKeyPairSigner,
    getBase64EncodedWireTransaction,
    getCompiledTransactionMessageDecoder,
    getSignatureFromTransaction,
    lamports,
    pipe,
    setTransactionMessageFeePayerSigner,
    signTransactionMessageWithSigners,
} from '@solana/kit';
import { isFailedTransaction, litesvm } from '@solana/kit-plugin-litesvm';
import { airdropPayer, generatedPayer } from '@solana/kit-plugin-signer';
import {
    findAssociatedTokenPda,
    getCreateAssociatedTokenIdempotentInstructionAsync,
    getTokenDecoder,
    getTransferCheckedInstruction,
    TOKEN_PROGRAM_ADDRESS,
    tokenProgram,
} from '@solana-program/token';
import { describe, expect, it, vi } from 'vitest';
import { writeFileSync } from 'node:fs';

import { validateTransfer } from '../src/validateTransfer.js';

// No external RPC, wallet, production mint or network. Actual signed transfers
// execute in LiteSVM. Its lack of getTransaction RPC is bridged with documented
// RPC metadata derived from real account snapshots (not fabricated balances).
async function executedTransfer(createRecipientInPayment: boolean) {
    const client = await createClient()
        .use(await generatedPayer())
        .use(litesvm())
        .use(airdropPayer(lamports(100_000_000_000n)))
        .use(tokenProgram());
    const payer = client.payer;
    const mint = await generateKeyPairSigner();
    const recipient = await generateKeyPairSigner();
    const reference = (await generateKeyPairSigner()).address;
    const decimals = 6;
    const amount = 1_000_000n;
    await client.token.instructions
        .createMint({
            payer,
            newMint: mint,
            decimals,
            mintAuthority: payer.address,
        })
        .sendTransaction();
    await client.token.instructions
        .mintToATA({
            payer,
            mint: mint.address,
            amount: amount * 2n,
            decimals,
            mintAuthority: payer,
            owner: payer.address,
        })
        .sendTransaction();
    const [source] = await findAssociatedTokenPda({
        owner: payer.address,
        mint: mint.address,
        tokenProgram: TOKEN_PROGRAM_ADDRESS,
    });
    const [destination] = await findAssociatedTokenPda({
        owner: recipient.address,
        mint: mint.address,
        tokenProgram: TOKEN_PROGRAM_ADDRESS,
    });
    const createATA = await getCreateAssociatedTokenIdempotentInstructionAsync({
        payer,
        owner: recipient.address,
        mint: mint.address,
    });
    if (!createRecipientInPayment) await client.sendTransaction([createATA]);

    const transfer = getTransferCheckedInstruction({
        source,
        destination,
        mint: mint.address,
        authority: payer,
        amount,
        decimals,
    });
    const message = pipe(
        createTransactionMessage({ version: 0 }),
        m => setTransactionMessageFeePayerSigner(payer, m),
        m => client.svm.setTransactionMessageLifetimeUsingLatestBlockhash(m),
        m =>
            appendTransactionMessageInstructions(
                [
                    ...(createRecipientInPayment ? [createATA] : []),
                    {
                        ...transfer,
                        accounts: [...transfer.accounts, { address: reference, role: AccountRole.READONLY }],
                    },
                ],
                m,
            ),
    );
    const transaction = await signTransactionMessageWithSigners(message);
    const accounts = getCompiledTransactionMessageDecoder().decode(transaction.messageBytes).staticAccounts;
    const destinationIndex = accounts.indexOf(destination);
    expect(destinationIndex).toBeGreaterThanOrEqual(0);
    const tokenSnapshot = () =>
        accounts.flatMap((account: Address, accountIndex: number) => {
            const state = client.svm.getAccount(account);
            if (!state.exists || state.programAddress !== TOKEN_PROGRAM_ADDRESS || state.data.length !== 165) return [];
            const token = getTokenDecoder().decode(state.data);
            return [
                {
                    accountIndex,
                    mint: token.mint,
                    owner: token.owner,
                    programId: TOKEN_PROGRAM_ADDRESS,
                    uiTokenAmount: { amount: token.amount.toString(), decimals },
                },
            ];
        });
    const preBalances = accounts.map(account => client.svm.getBalance(account) ?? lamports(0n));
    const preTokenBalances = tokenSnapshot();
    const before = client.svm.getAccount(destination);
    expect(before.exists).toBe(!createRecipientInPayment);
    const result = client.svm.sendTransaction(transaction);
    expect(isFailedTransaction(result), result.toString()).toBe(false);
    if (isFailedTransaction(result)) throw new Error(result.toString());
    const recipientAccount = await client.token.accounts.token.fetch(destination);
    expect(recipientAccount.data.amount).toBe(amount);
    const postTokenBalances = tokenSnapshot();
    const response = {
        meta: {
            err: null,
            preBalances,
            postBalances: accounts.map(account => client.svm.getBalance(account) ?? lamports(0n)),
            preTokenBalances,
            postTokenBalances,
            logMessages: result.logs(),
            loadedAddresses: { writable: [], readonly: [] },
        },
        transaction: [getBase64EncodedWireTransaction(transaction), 'base64'],
    };
    const signature = getSignatureFromTransaction(transaction);
    const rpc = {
        getTransaction: vi.fn().mockReturnValue({ send: vi.fn().mockResolvedValue(response) }),
    } as unknown as Rpc<GetTransactionApi>;
    const fields = { recipient: recipient.address, amount: 1, splToken: mint.address, reference };
    if (createRecipientInPayment && process.env.PAY_ATA_EVIDENCE_FILE) {
        writeFileSync(
            process.env.PAY_ATA_EVIDENCE_FILE,
            JSON.stringify(
                {
                    signature,
                    fields,
                    response,
                    provenance:
                        'signed transaction executed in local LiteSVM; RPC metadata assembled from before/after snapshots',
                },
                (_key, value) => (typeof value === 'bigint' ? value.toString() : value),
                2,
            ) + '\n',
        );
    }
    console.log(
        'LOCAL_SVM_EVIDENCE',
        JSON.stringify({
            createRecipientInPayment,
            signature,
            destination,
            destinationIndex,
            recipientPreLamports: preBalances[destinationIndex].toString(),
            recipientPreTokenEntry: preTokenBalances.find(b => b.accountIndex === destinationIndex) ?? null,
            recipientPostTokenEntry: postTokenBalances.find(b => b.accountIndex === destinationIndex),
            svmSuccess: !isFailedTransaction(result),
        }),
    );
    return { rpc, response, signature, fields };
}

describe('validateTransfer after actual local SPL execution', () => {
    it('CONTROL: accepts the payment when the recipient ATA already existed', async () => {
        const { rpc, response, signature, fields } = await executedTransfer(false);
        await expect(validateTransfer(rpc, signature, fields)).resolves.toBe(response);
    });

    it('REGRESSION: accepts a successful payment that creates the recipient ATA', async () => {
        const { rpc, response, signature, fields } = await executedTransfer(true);
        await expect(validateTransfer(rpc, signature, fields)).resolves.toBe(response);
    });

    it('CONTROL: rejects insufficient payment for an existing ATA', async () => {
        const { rpc, signature, fields } = await executedTransfer(false);
        await expect(validateTransfer(rpc, signature, { ...fields, amount: 2 })).rejects.toThrow(
            'amount not transferred',
        );
    });

    it('CONTROL: rejects an incorrect reference for an existing ATA', async () => {
        const { rpc, signature, fields } = await executedTransfer(false);
        const wrongReference = (await generateKeyPairSigner()).address;
        await expect(validateTransfer(rpc, signature, { ...fields, reference: wrongReference })).rejects.toThrow(
            'invalid reference 0',
        );
    });
});
