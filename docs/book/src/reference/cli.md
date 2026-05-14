# CLI reference

| Command | Description |
|---|---|
| *(none)* | Open the interactive TUI menu |
| `configure` | Edit settings in a terminal UI |
| `setup [USERNAME]` | Guided first-time setup (camera → enrol → PAM wiring) |
| `status` | Compact installation, broker reachability, and enrolment summary |
| `logs [--lines N]` | Show the current user's local diagnostic log |
| `emergency-disable [--dry-run]` | Restore Facegate PAM backups, remove remaining Facegate PAM lines, stop services |
| `users [--json]` | List enrolled users and broker storage ownership state |
| `broker status` | Show broker service, socket, audit log, and storage status |
| `broker health` | Ping the broker over IPC and print version/protocol |
| `broker restart` | Restart `facegate-brokerd.service` |
| `broker logs [--lines N]` | Show recent `facegate-brokerd.service` journal lines |
| `broker repair-permissions` | Re-apply `facegate:facegate` ownership and private modes |
| `doctor` | Check installation status |
| `cameras` | List `/dev/video*` and flag IR vs RGB |
| `camera-test [--device DEV]` | Test camera and face detection |
| `add USERNAME [--label LABEL] [--for sudo\|session\|both]` | Enrol face templates |
| `list USERNAME` | List enrolled templates |
| `remove USERNAME ID` | Remove a template by ID |
| `forget USERNAME [--yes]` | Remove every enrolled template for a user |
| `test USERNAME [--for sudo\|session\|all]` | Live recognition test |
| `calibrate USERNAME [--for sudo\|session] [--samples N] [--write]` | Recommend a recognition threshold from live positive samples |
| `calibrate-cameras [--rgb-device DEV] [--ir-device DEV] [--samples N] [--write] [--enable]` | Estimate the IR→RGB homography for cross-check |
| `session-auth` | Toggle face auth in login/session PAM services |
| `completions SHELL` | Print shell completion script |

## Privilege requirements

| No root needed | `completions`, `cameras`, `status`, `logs`, `users`, `broker status`, `broker health`, `broker logs`, plus the internal `watch` and `auth` helpers |
| Root needed | Everything else — enrolment, PAM edits, broker restart, configure, setup, calibration writes |

If PAM recovery is needed, start with `sudo facegate emergency-disable
--dry-run`; see [Recovery and emergency
disable](../security/recovery.md) for the full procedure.

## Local diagnostic log

Every CLI invocation that opens a camera or talks to the broker also
writes a coarse line to `~/.local/state/facegate/facegate.log`:
camera errors, timeouts, cross-check rejects, match scores, and
accept/reject outcomes. It contains no frames or embeddings. Use
`facegate logs` to view recent lines.
