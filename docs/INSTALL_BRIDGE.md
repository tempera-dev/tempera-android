# Install the fast bridge

With an emulator or a device connected over ADB:

```bash
tempera-android --serial emulator-5554 bridge setup
tempera-android --serial emulator-5554 bridge status --json
```

The setup command builds the companion APK from source using the installed Android SDK, installs it, provisions a per-emulator host token, enables the Accessibility service when the emulator permits secure-setting automation, creates an ADB forward, and verifies the bridge protocol.

The published release also includes `tempera-android-bridge.apk`; pass its
verified path with `bridge setup --apk PATH` when building the companion locally
is not appropriate for the host.

If Android requires a one-time manual Accessibility confirmation, setup reports that requirement explicitly. On physical devices, the owner must enable **Tempera Android Bridge** in Accessibility settings; the engine never attempts to bypass that confirmation.

To force a particular control plane:

```bash
tempera-android --serial emulator-5554 --transport bridge snapshot --json
tempera-android --serial emulator-5554 --transport adb snapshot --json
```

The default `--transport auto` prefers the native bridge and falls back to the independent ADB/UIAutomator path.
