# Public protocol

The public control schema is `tempera.android.control/v1`. It defines `SnapshotV1`, `ActionV1`, `ActionReceiptV1`, and `SessionV1` in the Rust crate. Snapshots expose only compact `@eN` references. A native bridge's internal reference is intentionally withheld from serialization and can be used only at the observed revision.

`ActionV1.expectedRevision` and `expectedStateHash` bind an action to the semantic state that planned it. `batch` requires both values on every item and they must agree. Bridge `act_observe` sends the expected revision to the device before invoking any action, so a stale batch is rejected without side effects.

MCP accepts JSON-RPC over stdio and maps every tool to `CommandRequest`, the same structure used by the CLI and local JSONL daemon. This is intentional parity, not a parallel controller.

`stream` is a bounded, read-only `CommandRequest` that reuses the public
`snapshot` command for each event. It returns `tempera.android.stream/v1`
events, so observation revision, backend selection, and persisted inspector
state stay identical to normal control observations. `record` serializes the
same sanitized events as JSONL under `tempera.android.record/v1`; it is kept
CLI-only because choosing a local output path is a filesystem write, not a
default agent capability.
