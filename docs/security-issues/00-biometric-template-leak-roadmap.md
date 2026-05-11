# [Security] Block same-UID exfiltration of enrolled face templates

## Priority

Critical / P0.

This should be handled before anti-spoofing, threshold tuning, or other hardening work. Face templates are biometric secrets: unlike passwords, leaked embeddings cannot be rotated in any meaningful way by the user.

## Problem

Facegate currently stores enrolled ArcFace embeddings in:

```text
/var/lib/facegate/users/<username>/embeddings.json
```

The file is `0600`, but for session-auth flows it is owned by the enrolled user so `facegate-watch` can read it. That means any process running as the same user can also read the template:

```text
cat /var/lib/facegate/users/$USER/embeddings.json
```

This includes compromised browsers, malicious editor extensions, npm/pypi/cargo postinstall scripts, desktop malware, and any other same-UID code.

## Current architecture

- `pam_facegate.so` runs in the PAM caller and spawns `/usr/bin/facegate auth --user <name>`.
- `facegate auth` loads templates directly from `TemplateStore`.
- `facegate-watch` runs as a systemd user service.
- `facegate-watch` also loads templates directly from `TemplateStore`.
- Enrollment writes templates as root and then `chown`s session-capable templates to the enrolled user so the watch daemon can read them.

The result is a missing trust boundary: the component that holds biometric templates is in the same UID trust zone as the entire desktop session.

## Security goal

Move biometric templates behind a privileged local broker so that:

- user processes cannot read `embeddings.json`;
- `facegate-watch` cannot read raw embeddings;
- `facegate auth` cannot read raw embeddings;
- clients can only submit a live captured embedding or frame and receive a decision;
- template create/list/remove operations are mediated by the broker;
- on-disk templates are owned by a dedicated unprivileged system account, not by the enrolled user;
- the design is close to the Windows Hello trust-boundary model, while staying compatible with Linux logind camera ACLs.

## Non-goals

- This does not solve photo, replay, or mask spoofing by itself.
- This does not provide Windows Hello hardware camera attestation.
- This does not prevent root from reading templates.
- This does not prevent an active same-UID attacker from trying to spoof the broker with synthetic camera input. That belongs to a liveness / anti-spoofing issue.

## Proposed issue split

1. Add `facegate-brokerd`, a system service running as a dedicated `facegate` user.
2. Move template storage ownership and all template reads/writes to the broker.
3. Replace direct template reads in PAM auth, watch, test, list, add, and remove with broker IPC.
4. Add migration and packaging support for existing installations.
5. Harden the broker runtime and update docs to stop implying Windows Hello-equivalent isolation until the broker exists.

## Acceptance criteria

- Fresh installs create a `facegate` system user.
- `/var/lib/facegate/users` is no longer traversable/readable by normal users.
- Existing enrolled templates are migrated to `facegate:facegate`.
- `facegate auth` and `facegate-watch` succeed without direct filesystem access to embeddings.
- Running as the enrolled user, `cat /var/lib/facegate/users/$USER/embeddings.json` fails with permission denied.
- Tests cover permission expectations and broker authorization rules.
- README and man page describe embeddings as sensitive biometric templates, not as non-reconstructable low-risk data.

