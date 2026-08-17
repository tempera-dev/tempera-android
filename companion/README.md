# Tempera Android Bridge

This is the optional `dev.tempera.android.bridge` Accessibility companion. It is a local-only performance path, not a general remote-control service.

The host reaches its loopback socket only through `adb forward`. Each request carries a per-device random token, a client id, and the current server epoch. Protocol v3 serializes actions, rejects stale revisions before a batch begins, redacts password values, and caches request receipts to provide at-most-once behavior on retries.

Build and install it with:

```bash
bash scripts/build-companion.sh
tempera-android --serial emulator-5554 bridge setup
tempera-android --serial emulator-5554 bridge status --json
```

The `TEMPERA_ANDROID_COMPILE_SDK` variable selects the Android compile surface (default: 36). Physical devices require the owner to manually authorize the Accessibility service. The bridge exposes no arbitrary shell action.
