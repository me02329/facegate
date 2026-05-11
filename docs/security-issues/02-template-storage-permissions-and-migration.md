# [Security] Move template ownership to `facegate:facegate` and migrate existing installs

## Priority

Critical / P0.

## Problem

Enrollment currently may `chown` template files to the enrolled user:

```rust
chown_user_data_dir(username, &config.storage.base_dir)
```

That is required today because `facegate-watch` is a user service and reads templates directly. Once `facegate-brokerd` exists, this ownership model should be removed.

## Target filesystem layout

```text
/var/lib/facegate
  owner: root:root
  mode: 0755

/var/lib/facegate/users
  owner: facegate:facegate
  mode: 0700

/var/lib/facegate/users/<username>
  owner: facegate:facegate
  mode: 0700

/var/lib/facegate/users/<username>/embeddings.json
  owner: facegate:facegate
  mode: 0600
```

Normal users must not be able to list, read, write, or replace their template directory.

## Packaging changes

In `packaging/nfpm/scripts/postinstall.sh`:

- create a system group `facegate`;
- create a system user `facegate` with no home and `nologin`;
- create `/run/facegate` through systemd runtime directory management, not manually in postinstall;
- create and fix ownership of `/var/lib/facegate/users`;
- migrate old template ownership from `<username>` to `facegate:facegate`;
- keep existing `embeddings.json` files at `0600`.

Expected user creation behavior:

```sh
getent group facegate >/dev/null || groupadd --system facegate
if [ -x /usr/sbin/nologin ]; then
    facegate_nologin=/usr/sbin/nologin
elif [ -x /sbin/nologin ]; then
    facegate_nologin=/sbin/nologin
else
    facegate_nologin=/bin/false
fi
id -u facegate >/dev/null 2>&1 || useradd --system --no-create-home --gid facegate --shell "$facegate_nologin" facegate
```

Do not hardcode a single `nologin` path. Debian/Ubuntu/Fedora commonly use `/usr/sbin/nologin`; Arch may use `/sbin/nologin`; `/bin/false` is an acceptable fallback.

## Code changes

- Remove or gate `chown_user_data_dir`.
- `facegate add --for session` must not transfer ownership to the enrolled user.
- `TemplateStore::ensure_base_dir` should accept `0700` for the broker-owned base directory.
- Add a startup or doctor check that reports unsafe legacy ownership.
- Add a migration command or package-time migration path.

## Migration safety

Migration must be conservative:

- reject symlinks;
- reject non-directories under `/var/lib/facegate/users`;
- reject group/world-writable paths;
- never follow untrusted paths while changing ownership;
- leave suspicious entries untouched and report them.

## Acceptance criteria

- Fresh install creates broker-owned template storage.
- Upgrade from current installs preserves templates and changes ownership to `facegate:facegate`.
- Same-UID read test fails:

```sh
cat /var/lib/facegate/users/$USER/embeddings.json
```

- `facegate-watch` continues to unlock via the broker.
- `facegate doctor` reports legacy user-owned templates as insecure.
