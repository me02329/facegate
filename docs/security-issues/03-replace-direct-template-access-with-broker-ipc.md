# [Security] Remove direct template access from CLI auth, watch, test, list, add, and remove

## Priority

Critical / P0.

## Problem

Several commands currently instantiate `TemplateStore` directly:

- `crates/facegate_cli/src/commands/auth.rs`
- `crates/facegate_cli/src/commands/watch.rs`
- `crates/facegate_cli/src/commands/add.rs`
- `crates/facegate_cli/src/commands/test.rs`
- `crates/facegate_cli/src/commands/list.rs`
- `crates/facegate_cli/src/commands/remove.rs`
- TUI flows that call those helpers

After adding `facegate-brokerd`, these commands must stop reading enrolled embeddings directly.

## Target behavior

### `facegate auth`

Current:

```text
load enrolled templates -> capture probe embedding -> compare locally
```

Target:

```text
capture probe embedding -> broker Match(username, scope, probe_embedding) -> exit code
```

### `facegate-watch`

Current:

```text
load enrolled templates -> capture probe embedding -> compare locally -> Unlock()
```

Target:

```text
capture probe embedding -> broker Match(username, session, probe_embedding) -> Unlock()
```

### `facegate add`

Current:

```text
capture enrollment embedding -> TemplateStore::add_template
```

Target:

```text
capture enrollment embedding -> broker Enroll(...)
```

### `facegate list`

Current:

```text
TemplateStore::load returns templates including embeddings
```

Target:

```text
broker List returns metadata only
```

### `facegate test`

Current:

```text
capture probe embedding -> load all enrolled embeddings -> print best similarity
```

Target:

```text
capture probe embedding -> broker Match/TestMatch -> print metadata and score if allowed
```

Do not expose stored vectors. If debug scores are needed, return only aggregate match information.

## Acceptance criteria

- `rg "TemplateStore::new" crates/facegate_cli` only finds broker-admin/internal paths, not auth/watch/list/add/remove logic.
- No user-facing command can print, serialize, or return enrolled embedding vectors.
- `facegate list` shows id, label, scope, and created_at only.
- PAM auth still falls through according to existing exit-code behavior.
- Watch unlock behavior is unchanged from the user's perspective.
- Tests cover broker unavailable behavior:
  - PAM helper fails closed or falls back according to config.
  - Watch logs and skips unlock.
  - Admin commands show actionable errors.

