# Persistent bridge validation

The native Accessibility bridge hot path must keep the host control socket open across agent turns. Protocol v3 response deduplication is keyed by `(client_id, request_id)`, so a reconnect retry must resend the identical request ID rather than create a second mutation.

Validation requirements:

- one ADB forward per bridge client;
- one hot TCP channel across observations and actions when healthy;
- `TCP_NODELAY` plus bounded read/write timeouts;
- same-ID single reconnect after an ambiguous transport failure;
- stale revision and server-epoch checks remain device-side authority;
- no retry with a new request ID after mutation delivery becomes ambiguous.
