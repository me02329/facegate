# [Security] Add TPM2 sealing for broker-owned biometric templates

## Priority

Medium/high, after broker-owned storage exists.

## Problem

Moving templates to `facegate:facegate` blocks normal user processes from reading them, but offline attackers and root-equivalent local compromise can still access `/var/lib/facegate/users`.

TPM sealing can raise the bar by encrypting stored templates with a key that is only released when the machine boots into an expected state.

## Security goal

Protect broker-owned templates at rest against offline disk access and some evil-maid scenarios, while keeping recovery and upgrade flows usable.

## Proposed approach

- Generate a random data-encryption key for Facegate template storage.
- Seal or wrap that key with TPM2.
- Bind unsealing to a conservative PCR policy.
- Encrypt `embeddings.json` or replace it with an encrypted store format.
- Keep the broker as the only process that can unseal and decrypt templates.

## Design requirements

- Recovery flow for firmware, bootloader, kernel, and initramfs updates that change PCRs.
- Clear fallback behavior when TPM is unavailable.
- Explicit migration path from plaintext broker-owned templates.
- No user password reuse as the only encryption factor.
- Docs that explain when templates are protected and when they are not.

## Risks

PCR policies can be brittle. A firmware or bootloader update can make sealed keys unavailable. The implementation must include a re-seal/recovery procedure before this ships as a default.

## Acceptance criteria

- Templates can be encrypted at rest with a TPM-sealed key.
- Broker can decrypt templates only after successful unseal.
- Failure modes are explicit and actionable in `facegate doctor`.
- Upgrade and recovery flows are tested.
- Docs warn users before enabling TPM sealing on systems without a tested recovery path.

