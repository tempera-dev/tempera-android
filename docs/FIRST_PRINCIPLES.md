# First-principles Android computer use

The system is optimized around one end-to-end quantity: **time and error probability per verified useful state transition**.

A computer-use step is decomposed into four costs:

1. **Observe** — acquire enough state to choose the next action.
2. **Reason** — decide what to do from that state.
3. **Act** — execute the action with the least indirection possible.
4. **Verify** — establish what state actually resulted before planning again.

The architecture minimizes each term independently and then fuses the boundaries between them.

## Observe

The preferred path is the companion Accessibility service, not UIAutomator polling. Android already maintains a semantic accessibility tree and emits events when that tree or focused window changes. The bridge serializes that state directly over a persistent local socket.

The planner does not receive the whole tree by default. The host ranks nodes against task tokens and actionability and sends a small task-focused semantic view first. It escalates only when required:

`task-ranked semantics -> full semantics -> screenshot`

This reduces both host serialization and model-prefill cost while retaining a complete fallback.

The ADB/UIAutomator implementation remains available as a zero-install fallback and as an independent benchmark baseline.

## Reason

The planner sees stable semantic references, widget capabilities, resource IDs, labels, geometry, focused/editable state, and a compact history. Pixel inference is reserved for controls that do not expose useful semantics.

A separate vision model may be configured so a fast text-capable planner handles ordinary UI states while a multimodal model is paid only for visual ambiguity.

## Act

Accessibility-native actions are preferred over coordinate injection:

- node `ACTION_CLICK`
- node `ACTION_LONG_CLICK`
- node `ACTION_SET_TEXT`
- semantic scrolling
- IME enter
- Android global back/home/recents/notification actions
- `dispatchGesture` only when semantic action support is absent

Actions may be batched when later actions do not require a newly observed selector. The ADB fallback compiles those actions into one device-side shell transaction.

## Verify

Every native observation has a monotonically increasing UI revision. An action request carries the revision it was planned against. If Android changed before execution, the bridge rejects the entire stale batch and returns fresh state without clicking anything.

`act_observe` fuses execution, event waiting, and next-state capture into one transport request. The agent therefore carries the returned observation into the next reasoning step instead of performing a second poll.

## Hot path

With the bridge enabled, the intended loop is:

```text
Accessibility events/tree
        |
 task-ranked state
        |
      planner
        |
 revision-bound batch
        |
  native Android action
        |
 wait for state event
        |
 next semantic state
```

That makes ADB a bootstrap/forwarding/fallback transport rather than the per-action control mechanism.

## Safety properties

- Password text is redacted in the on-device semantic encoder.
- The bridge listens only on the Android loopback interface and is reached through `adb forward`.
- Requests require a random host-generated token stored in the companion app's private data directory.
- Arbitrary shell execution is not part of the autonomous bridge protocol.
- Stale revisions fail closed.
- Consequential UI labels remain approval-gated in the autonomous planner path.
- Vision and ADB are fallbacks, not mechanisms for bypassing app or device integrity controls.

## Measurement

`tempera-android bench` reports observation latency, semantic payload size, sequential control overhead, batched control overhead, and fused act-observe latency. When the native bridge is active it also runs the ADB/UIAutomator baseline and reports relative speedups.

The benchmark deliberately uses harmless zero-duration waits for control-plane measurements. End-to-end task quality and latency must be measured separately on real app tasks before making an empirical 10x claim.
