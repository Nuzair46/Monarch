# September 2026 review follow-up

This change addresses the 15 findings from the review of main at `7cb81539e37aedff3687c5b0c6c5575427dad7a4`. Implementation details and limitations are in [architecture.md](architecture.md); user-visible changes are in [CHANGELOG.md](../CHANGELOG.md).

| Finding | Implemented change | Regression evidence |
| --- | --- | --- |
| 1. Incompatible desktop lockfile | Tauri runtime family aligned; builds use committed locks | Windows target source/test compilation and Clippy |
| 2. Failed mutation escapes recovery | Journal and pending state established before apply; unresolved failures keep recovery | Fault-injected partial mutation and rollback failures |
| 3. Restart loses unconfirmed transaction | Persistent journal reconstructed as expired on startup | Manager restart restores previous layout |
| 4. Corrupt writes and live-state divergence | Atomic replacement, valid backup, schema guard, clone/save/commit mutators | Backup corruption, save/delete/settings failures, future-schema rejection |
| 5. Localized startup deletion failure | Numeric registry APIs; synchronize only changed startup preference on settings save | Isolated Windows registry fixture test compiled; Windows execution still needed |
| 6. Slideshow/background resets | Capture background mode/options and restore only changed state | Windows source compilation; slideshow/disabled-background hardware acceptance still needed |
| 7. Connection confused with physical identity | Shared evidence-based resolver; conflicting serials and ambiguous matches rejected | Port/adapter changes, duplicate serials, metadata loss, EDID validation |
| 8. Unsafe native byte persistence | Typed, bounded geometry history; native routes ephemeral | History round trip and reused-endpoint tests; removed raw codec |
| 9. Inconsistent display snapshots | One inventory query, generation-tagged published snapshot, distinct history | Coherent backend implementation and history-does-not-invent-monitors test |
| 10. Global lock and unbounded threads | Bounded worker queue, published reads, coalesced refreshes, shared nonblocking recovery schedule | Queue saturation/read tests compiled; recovery retry tests executed |
| 11. IPC deadlines and cross-session scope | User/session named pipe, user-only ACL, bounded framing, acceptance/completion responses | Framing/deadline/disconnect tests executed in a portable harness; real pipe/session tests still needed |
| 12. Version bump leaves stale locks | Update local package entries in both locks; locked release tests/build; tag after build | Disposable release fixture passes locked Cargo check without dependency changes |
| 13. Incomplete layout model/late validation | Rotation, explicit automatic mode preference, bounds/uniqueness validation, cloned-layout rejection | Rotation round trip/verification, malformed layout rejected before journaling |
| 14. Late listener registration leaks | Subscription owner disposes registrations even after cleanup | Asynchronous registration/unmount/rejection tests |
| 15. COM initialization/drop order | Apartment guard outlives interfaces and covers failed construction | RAII implementation and Windows source compilation |

Cleanup also removes the skeleton backend, unreachable stub, obsolete attachment candidates and duplicate matching policy, unused Tauri read commands, redundant fingerprint strings, and unused frontend packages. Custom shortcut mode is preserved; the UI writes only the selected base and existing custom mappings. The browser mock has its own module and confirmation-contract tests.

Local validation uses a Linux host. Core tests, frontend tests/build, release fixtures and portable IPC tests execute here. Windows target checking skips Tauri resource generation and compiles Windows tests without executing them. Full Windows executable/MSI builds, real registry/named-pipe execution and physical display acceptance are separate requirements. CI is left to the maintainer; this work does not wait for or claim its results.
