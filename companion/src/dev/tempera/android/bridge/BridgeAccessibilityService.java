package dev.tempera.android.bridge;

import android.accessibilityservice.AccessibilityService;
import android.accessibilityservice.AccessibilityServiceInfo;
import android.accessibilityservice.GestureDescription;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Path;
import android.graphics.Rect;
import android.hardware.HardwareBuffer;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.util.Base64;
import android.view.Display;
import android.view.accessibility.AccessibilityEvent;
import android.view.accessibility.AccessibilityNodeInfo;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.util.concurrent.Callable;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.FutureTask;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicLong;
import java.util.concurrent.atomic.AtomicReference;

public final class BridgeAccessibilityService extends AccessibilityService {
    private final AtomicLong revision = new AtomicLong(1L);
    private final AtomicLong lastStateEventNanos = new AtomicLong(System.nanoTime());
    private final Object revisionMonitor = new Object();
    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private final ExecutorService screenshotExecutor = Executors.newSingleThreadExecutor();

    private volatile String packageName = "";
    private volatile String className = "";
    private volatile BridgeServer server;

    @Override
    protected void onServiceConnected() {
        super.onServiceConnected();
        AccessibilityServiceInfo info = getServiceInfo();
        info.flags |= AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS;
        info.flags |= AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS;
        info.flags |= AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS;
        setServiceInfo(info);
        server = new BridgeServer(this);
        server.start();
    }

    @Override
    public void onAccessibilityEvent(AccessibilityEvent event) {
        if (event == null) return;
        if (event.getPackageName() != null) packageName = event.getPackageName().toString();
        if (event.getClassName() != null) className = event.getClassName().toString();
        if (!isStateEvent(event.getEventType())) return;
        synchronized (revisionMonitor) {
            revision.incrementAndGet();
            lastStateEventNanos.set(System.nanoTime());
            revisionMonitor.notifyAll();
        }
    }

    @Override
    public void onInterrupt() {
        // No audio or spoken feedback is used by the bridge.
    }

    @Override
    public void onDestroy() {
        BridgeServer current = server;
        if (current != null) current.close();
        screenshotExecutor.shutdownNow();
        super.onDestroy();
    }

    long currentRevision() {
        return revision.get();
    }

    String currentPackage() {
        return packageName;
    }

    String currentClassName() {
        return className;
    }

    int screenWidth() {
        return getResources().getDisplayMetrics().widthPixels;
    }

    int screenHeight() {
        return getResources().getDisplayMetrics().heightPixels;
    }

    String readToken() {
        try {
            File tokenFile = new File(getFilesDir(), "bridge.token");
            if (!tokenFile.isFile()) return "";
            return new String(Files.readAllBytes(tokenFile.toPath()), StandardCharsets.UTF_8).trim();
        } catch (Exception ignored) {
            return "";
        }
    }

    JSONObject observe() throws Exception {
        return callOnMain(() -> NodeCodec.snapshot(this));
    }

    JSONObject transitionNow(long previous) throws Exception {
        long current = revision.get();
        JSONObject out = new JSONObject();
        out.put("changed", current > previous);
        out.put("settled", true);
        out.put("events", Math.max(0L, current - previous));
        out.put("revision", current);
        out.put("quiet_ms", 0L);
        out.put("wait_ms", 0.0);
        return out;
    }

    JSONObject waitForSettledRevision(
            long previous,
            long firstChangeTimeoutMs,
            long quietMs,
            long maxSettleMs
    ) throws Exception {
        long started = System.nanoTime();
        long firstDeadline = started + TimeUnit.MILLISECONDS.toNanos(Math.max(0L, firstChangeTimeoutMs));
        long settleDeadline = Long.MAX_VALUE;
        boolean changed = revision.get() > previous;
        boolean settled = false;

        synchronized (revisionMonitor) {
            while (!changed) {
                long now = System.nanoTime();
                long remaining = firstDeadline - now;
                if (remaining <= 0L) break;
                TimeUnit.NANOSECONDS.timedWait(revisionMonitor, remaining);
                changed = revision.get() > previous;
            }

            if (changed) {
                settleDeadline = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(Math.max(quietMs, maxSettleMs));
                long quietNanos = TimeUnit.MILLISECONDS.toNanos(Math.max(0L, quietMs));
                while (true) {
                    long now = System.nanoTime();
                    long quietDeadline = lastStateEventNanos.get() + quietNanos;
                    if (now >= quietDeadline) {
                        settled = true;
                        break;
                    }
                    if (now >= settleDeadline) break;
                    long remaining = Math.min(quietDeadline, settleDeadline) - now;
                    if (remaining <= 0L) continue;
                    TimeUnit.NANOSECONDS.timedWait(revisionMonitor, remaining);
                }
            } else {
                settled = true;
            }
        }

        long current = revision.get();
        JSONObject out = new JSONObject();
        out.put("changed", changed);
        out.put("settled", settled);
        out.put("events", Math.max(0L, current - previous));
        out.put("revision", current);
        out.put("quiet_ms", quietMs);
        out.put("wait_ms", (System.nanoTime() - started) / 1_000_000.0);
        return out;
    }

    JSONArray executeActions(JSONArray actions) throws Exception {
        JSONArray results = new JSONArray();
        for (int index = 0; index < actions.length(); index++) {
            JSONObject action = actions.optJSONObject(index);
            long started = System.nanoTime();
            JSONObject result = new JSONObject();
            if (action == null) {
                result.put("ok", false);
                result.put("error", "action must be an object");
                result.put("latency_ms", (System.nanoTime() - started) / 1_000_000.0);
                results.put(result);
                break;
            }
            result.put("action", action);
            try {
                String detail = executeAction(action);
                result.put("ok", true);
                result.put("detail", detail);
            } catch (Exception exc) {
                result.put("ok", false);
                result.put("error", safeMessage(exc));
            }
            result.put("latency_ms", (System.nanoTime() - started) / 1_000_000.0);
            results.put(result);
            if (!result.optBoolean("ok", false)) break;
        }
        return results;
    }

    private String executeAction(JSONObject action) throws Exception {
        String type = action.optString("type", "");
        switch (type) {
            case "tap":
                return tap(action);
            case "long_press":
                return longPress(action);
            case "type":
                return setText(action);
            case "back":
                return global(GLOBAL_ACTION_BACK, "back");
            case "home":
                return global(GLOBAL_ACTION_HOME, "home");
            case "recents":
                return global(GLOBAL_ACTION_RECENTS, "recents");
            case "notifications":
                return global(GLOBAL_ACTION_NOTIFICATIONS, "notifications");
            case "enter":
                return imeEnter(action);
            case "scroll":
                return scroll(action);
            case "swipe":
                return swipe(action);
            case "launch":
                return launch(action);
            case "wait":
                long millis = Math.max(0L, Math.min(10_000L, Math.round(action.optDouble("seconds", 0.25) * 1000.0)));
                Thread.sleep(millis);
                return "wait " + millis + "ms";
            case "key":
                throw new IllegalArgumentException("arbitrary key injection is intentionally host-ADB fallback only");
            default:
                throw new IllegalArgumentException("unsupported action type: " + type);
        }
    }

    private String tap(JSONObject action) throws Exception {
        String ref = action.optString("ref", "");
        if (!ref.isEmpty()) {
            AccessibilityNodeInfo node = callOnMain(() -> NodeCodec.findByRef(this, ref));
            if (node == null) throw new IllegalArgumentException("stale or missing ref: " + ref);
            try {
                String label = nodeLabel(node);
                boolean clicked = callOnMain(() -> node.performAction(AccessibilityNodeInfo.ACTION_CLICK));
                if (clicked) return label;
                Rect bounds = new Rect();
                node.getBoundsInScreen(bounds);
                gesture(bounds.centerX(), bounds.centerY(), bounds.centerX(), bounds.centerY(), 50L);
                return label + " (gesture fallback)";
            } finally {
                node.recycle();
            }
        }
        int x = action.getInt("x");
        int y = action.getInt("y");
        validatePoint(x, y);
        gesture(x, y, x, y, 50L);
        return x + "," + y;
    }

    private String longPress(JSONObject action) throws Exception {
        String ref = action.optString("ref", "");
        long duration = Math.max(350L, Math.min(5_000L, action.optLong("duration_ms", 700L)));
        if (!ref.isEmpty()) {
            AccessibilityNodeInfo node = callOnMain(() -> NodeCodec.findByRef(this, ref));
            if (node == null) throw new IllegalArgumentException("stale or missing ref: " + ref);
            try {
                String label = nodeLabel(node);
                boolean performed = callOnMain(() -> node.performAction(AccessibilityNodeInfo.ACTION_LONG_CLICK));
                if (performed) return label;
                Rect bounds = new Rect();
                node.getBoundsInScreen(bounds);
                gesture(bounds.centerX(), bounds.centerY(), bounds.centerX(), bounds.centerY(), duration);
                return label + " (gesture fallback)";
            } finally {
                node.recycle();
            }
        }
        int x = action.getInt("x");
        int y = action.getInt("y");
        validatePoint(x, y);
        gesture(x, y, x, y, duration);
        return "long press";
    }

    private String setText(JSONObject action) throws Exception {
        String value = action.optString("text", "");
        String ref = action.optString("ref", "");
        AccessibilityNodeInfo node = !ref.isEmpty()
                ? callOnMain(() -> NodeCodec.findByRef(this, ref))
                : callOnMain(this::focusedInputNode);
        if (node == null) throw new IllegalArgumentException("no editable input target");
        try {
            if (node.isPassword() && action.optBoolean("read_modify_write", false)) {
                throw new IllegalArgumentException("read/modify/write is disabled for password fields");
            }
            Bundle args = new Bundle();
            args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, value);
            boolean ok = callOnMain(() -> node.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args));
            if (!ok) throw new IllegalArgumentException("target does not support ACTION_SET_TEXT");
            return value.length() + " chars";
        } finally {
            node.recycle();
        }
    }

    private String imeEnter(JSONObject action) throws Exception {
        String ref = action.optString("ref", "");
        AccessibilityNodeInfo node = !ref.isEmpty()
                ? callOnMain(() -> NodeCodec.findByRef(this, ref))
                : callOnMain(this::focusedInputNode);
        if (node == null) throw new IllegalArgumentException("no focused editable node for IME enter");
        try {
            int enterId = AccessibilityNodeInfo.AccessibilityAction.ACTION_IME_ENTER.getId();
            boolean ok = callOnMain(() -> node.performAction(enterId));
            if (!ok) throw new IllegalArgumentException("focused node does not expose ACTION_IME_ENTER");
            return "ime enter";
        } finally {
            node.recycle();
        }
    }

    private AccessibilityNodeInfo focusedInputNode() {
        AccessibilityNodeInfo root = getRootInActiveWindow();
        if (root == null) return null;
        AccessibilityNodeInfo focused = root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
        if (focused == null) {
            root.recycle();
            return null;
        }
        if (focused != root) root.recycle();
        return focused;
    }

    private String scroll(JSONObject action) throws Exception {
        String direction = action.optString("direction", "down");
        String ref = action.optString("ref", "");
        if (!ref.isEmpty()) {
            AccessibilityNodeInfo node = callOnMain(() -> NodeCodec.findByRef(this, ref));
            if (node == null) throw new IllegalArgumentException("stale or missing ref: " + ref);
            try {
                int actionId = direction.equals("up") || direction.equals("left")
                        ? AccessibilityNodeInfo.ACTION_SCROLL_BACKWARD
                        : AccessibilityNodeInfo.ACTION_SCROLL_FORWARD;
                boolean ok = callOnMain(() -> node.performAction(actionId));
                if (ok) return "semantic scroll " + direction;
            } finally {
                node.recycle();
            }
        }
        int width = screenWidth();
        int height = screenHeight();
        int x = width / 2;
        int hi = (int) (height * 0.78);
        int lo = (int) (height * 0.26);
        if (direction.equals("up")) gesture(x, lo, x, hi, 180L);
        else if (direction.equals("left")) gesture((int) (width * 0.24), height / 2, (int) (width * 0.78), height / 2, 180L);
        else if (direction.equals("right")) gesture((int) (width * 0.78), height / 2, (int) (width * 0.24), height / 2, 180L);
        else gesture(x, hi, x, lo, 180L);
        return "gesture scroll " + direction;
    }

    private String swipe(JSONObject action) throws Exception {
        int x1 = action.getInt("x1");
        int y1 = action.getInt("y1");
        int x2 = action.getInt("x2");
        int y2 = action.getInt("y2");
        validatePoint(x1, y1);
        validatePoint(x2, y2);
        long duration = Math.max(50L, Math.min(5_000L, action.optLong("duration_ms", 180L)));
        gesture(x1, y1, x2, y2, duration);
        return "swipe";
    }

    private String launch(JSONObject action) throws Exception {
        String target = action.getString("package");
        Intent intent = getPackageManager().getLaunchIntentForPackage(target);
        if (intent == null) throw new IllegalArgumentException("package has no launcher activity: " + target);
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK);
        callOnMain(() -> {
            startActivity(intent);
            return true;
        });
        return target;
    }

    private String global(int action, String label) throws Exception {
        boolean ok = callOnMain(() -> performGlobalAction(action));
        if (!ok) throw new IllegalArgumentException("global action unavailable: " + label);
        return label;
    }

    private void validatePoint(int x, int y) {
        if (x < 0 || y < 0 || x >= screenWidth() || y >= screenHeight()) {
            throw new IllegalArgumentException("gesture coordinate outside display bounds: " + x + "," + y);
        }
    }

    private void gesture(float x1, float y1, float x2, float y2, long durationMs) throws Exception {
        Path path = new Path();
        path.moveTo(x1, y1);
        if (x1 != x2 || y1 != y2) path.lineTo(x2, y2);
        GestureDescription gesture = new GestureDescription.Builder()
                .addStroke(new GestureDescription.StrokeDescription(path, 0L, durationMs))
                .build();
        boolean accepted = callOnMain(() -> dispatchGesture(gesture, null, null));
        if (!accepted) throw new IllegalArgumentException("Android rejected gesture dispatch");
        Thread.sleep(Math.min(durationMs + 20L, 5_050L));
    }

    String screenshotBase64() throws Exception {
        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<String> output = new AtomicReference<>();
        AtomicReference<Exception> failure = new AtomicReference<>();
        mainHandler.post(() -> takeScreenshot(Display.DEFAULT_DISPLAY, screenshotExecutor, new TakeScreenshotCallback() {
            @Override
            public void onSuccess(ScreenshotResult result) {
                HardwareBuffer buffer = result.getHardwareBuffer();
                try {
                    Bitmap hardware = Bitmap.wrapHardwareBuffer(buffer, result.getColorSpace());
                    if (hardware == null) throw new IllegalStateException("could not wrap screenshot buffer");
                    Bitmap software = hardware.copy(Bitmap.Config.ARGB_8888, false);
                    hardware.recycle();
                    if (software == null) throw new IllegalStateException("could not copy screenshot bitmap");
                    ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                    software.compress(Bitmap.CompressFormat.PNG, 100, bytes);
                    software.recycle();
                    output.set(Base64.encodeToString(bytes.toByteArray(), Base64.NO_WRAP));
                } catch (Exception exc) {
                    failure.set(exc);
                } finally {
                    buffer.close();
                    latch.countDown();
                }
            }

            @Override
            public void onFailure(int errorCode) {
                failure.set(new IllegalStateException("screenshot failed: " + errorCode));
                latch.countDown();
            }
        }));
        if (!latch.await(4, TimeUnit.SECONDS)) throw new IllegalStateException("screenshot timed out");
        if (failure.get() != null) throw failure.get();
        return output.get();
    }

    private <T> T callOnMain(Callable<T> callable) throws Exception {
        if (Looper.myLooper() == Looper.getMainLooper()) return callable.call();
        FutureTask<T> future = new FutureTask<>(callable);
        mainHandler.post(future);
        return future.get(4, TimeUnit.SECONDS);
    }

    private static boolean isStateEvent(int type) {
        return type == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED
                || type == AccessibilityEvent.TYPE_WINDOWS_CHANGED
                || type == AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED
                || type == AccessibilityEvent.TYPE_VIEW_CLICKED
                || type == AccessibilityEvent.TYPE_VIEW_TEXT_CHANGED
                || type == AccessibilityEvent.TYPE_VIEW_SCROLLED
                || type == AccessibilityEvent.TYPE_VIEW_FOCUSED;
    }

    private static String nodeLabel(AccessibilityNodeInfo node) {
        if (!node.isPassword() && node.getText() != null && node.getText().length() > 0) return node.getText().toString();
        if (node.getContentDescription() != null && node.getContentDescription().length() > 0) return node.getContentDescription().toString();
        String viewId = node.getViewIdResourceName();
        return viewId == null ? "" : viewId;
    }

    private static String safeMessage(Exception exception) {
        String message = exception.getMessage();
        if (message == null || message.isEmpty()) return exception.getClass().getSimpleName();
        return message.length() > 500 ? message.substring(0, 500) : message;
    }
}
