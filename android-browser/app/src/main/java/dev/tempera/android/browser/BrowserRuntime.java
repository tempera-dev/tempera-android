package dev.tempera.android.browser;

import android.app.Activity;
import android.net.Uri;
import android.os.SystemClock;
import android.webkit.WebView;

import org.json.JSONException;
import org.json.JSONObject;
import org.json.JSONTokener;

import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicReference;

final class BrowserRuntime {
    private static final long EVALUATION_TIMEOUT_MS = 5_000;
    private static final long MAX_SETTLE_MS = 2_000;

    private final Activity activity;
    private final WebView webView;
    private long revision;
    private String lastStateHash = "";
    private String lastUrl = "about:blank";
    private boolean domRuntimeInstalled;

    BrowserRuntime(Activity activity, WebView webView) {
        this.activity = activity;
        this.webView = webView;
    }

    synchronized JSONObject health() throws JSONException {
        return new JSONObject()
            .put("schemaVersion", "tempera.android.browser.health/v1")
            .put("ok", true)
            .put("package", activity.getPackageName())
            .put("port", ControlServer.PORT)
            .put("url", lastUrl)
            .put("revision", revision)
            .put("domRuntimeInstalled", domRuntimeInstalled)
            .put("primaryTransport", "instrumented-webview-dom")
            .put("fallbackTransport", "tempera-android-accessibility");
    }

    synchronized JSONObject navigate(String rawUrl) throws Exception {
        Uri uri = Uri.parse(rawUrl == null ? "" : rawUrl.trim());
        String scheme = uri.getScheme();
        if (!("https".equalsIgnoreCase(scheme) || "about:blank".equals(rawUrl))) {
            throw new IllegalArgumentException("only HTTPS and about:blank navigation are enabled");
        }
        CountDownLatch latch = new CountDownLatch(1);
        activity.runOnUiThread(() -> {
            webView.loadUrl(rawUrl);
            latch.countDown();
        });
        if (!latch.await(2, TimeUnit.SECONDS)) {
            throw new IllegalStateException("navigation dispatch timed out");
        }
        lastUrl = rawUrl;
        revision += 1;
        lastStateHash = "";
        domRuntimeInstalled = false;
        return new JSONObject()
            .put("schemaVersion", "tempera.android.browser.navigation/v1")
            .put("accepted", true)
            .put("url", rawUrl)
            .put("revision", revision);
    }

    synchronized JSONObject snapshot() throws Exception {
        ensureDomRuntime();
        JSONObject snapshot = evaluateObject(DomProgram.snapshot());
        return decorateSnapshot(snapshot);
    }

    synchronized JSONObject action(JSONObject request) throws Exception {
        String kind = request.optString("kind", "tap");
        if ("back".equals(kind)) {
            return navigateBack(request);
        }
        ensureDomRuntime();
        JSONObject result = evaluateObject(
            "scroll".equals(kind) ? DomProgram.scroll(request) : DomProgram.action(request)
        );
        decorateNestedSnapshot(result, "before");
        decorateNestedSnapshot(result, "after");
        return result;
    }

    synchronized JSONObject actObserve(JSONObject request) throws Exception {
        JSONObject actionRequest = request.optJSONObject("action");
        if (actionRequest == null) {
            throw new IllegalArgumentException("act-observe requires an action object");
        }
        JSONObject action = action(actionRequest);
        if (!action.optBoolean("ok", false)) {
            return new JSONObject()
                .put("schemaVersion", "tempera.android.browser.act-observe/v1")
                .put("ok", false)
                .put("action", action)
                .put("observation", JSONObject.NULL);
        }

        JSONObject immediate = action.optJSONObject("after");
        String initialHash = immediate == null ? "" : immediate.optString("documentStateHash", "");
        long settleMs = Math.min(Math.max(request.optLong("settleMs", 48), 0), MAX_SETTLE_MS);
        JSONObject observation = immediate;
        if (settleMs > 0) {
            long deadline = SystemClock.elapsedRealtime() + settleMs;
            while (SystemClock.elapsedRealtime() < deadline) {
                SystemClock.sleep(8);
                JSONObject candidate = snapshot();
                observation = candidate;
                if (!candidate.optString("documentStateHash", "").equals(initialHash)) {
                    break;
                }
            }
        }
        return new JSONObject()
            .put("schemaVersion", "tempera.android.browser.act-observe/v1")
            .put("ok", true)
            .put("action", action)
            .put("observation", observation == null ? JSONObject.NULL : observation);
    }

    synchronized JSONObject waitFor(JSONObject request) throws Exception {
        String previousHash = request.optString("previousStateHash", "");
        String exactText = request.optString("exactText", "");
        long timeoutMs = Math.min(Math.max(request.optLong("timeoutMs", 1_000), 1), 10_000);
        long deadline = SystemClock.elapsedRealtime() + timeoutMs;
        JSONObject latest = null;
        while (SystemClock.elapsedRealtime() <= deadline) {
            latest = snapshot();
            boolean changed = !previousHash.isEmpty()
                && !previousHash.equals(latest.optString("documentStateHash", ""));
            boolean found = exactText.isEmpty() || containsExactText(latest, exactText);
            if ((previousHash.isEmpty() || changed) && found) {
                return new JSONObject()
                    .put("schemaVersion", "tempera.android.browser.wait/v1")
                    .put("matched", true)
                    .put("snapshot", latest);
            }
            SystemClock.sleep(16);
        }
        return new JSONObject()
            .put("schemaVersion", "tempera.android.browser.wait/v1")
            .put("matched", false)
            .put("snapshot", latest == null ? JSONObject.NULL : latest);
    }

    synchronized void onPageStarted(String url) {
        lastUrl = url == null ? "" : url;
        revision += 1;
        lastStateHash = "";
        domRuntimeInstalled = false;
    }

    synchronized void onPageFinished(String url) {
        lastUrl = url == null ? "" : url;
        revision += 1;
        lastStateHash = "";
        domRuntimeInstalled = false;
        // Pre-warm the semantic runtime as soon as the document finishes. This
        // happens asynchronously on WebView's UI thread so page load is never
        // blocked by the host control channel. The first host request still
        // verifies/installs synchronously if it races this callback.
        activity.runOnUiThread(() -> webView.evaluateJavascript(DomProgram.install(), ignored -> {
            synchronized (BrowserRuntime.this) {
                domRuntimeInstalled = true;
            }
        }));
    }

    private JSONObject navigateBack(JSONObject request) throws Exception {
        String expected = request.optString("expectedStateHash", "");
        JSONObject before = snapshot();
        if (expected.isEmpty() || !expected.equals(before.optString("documentStateHash", ""))) {
            return new JSONObject()
                .put("ok", false)
                .put("stale", true)
                .put("error", "document state changed")
                .put("before", before);
        }
        AtomicReference<Boolean> moved = new AtomicReference<>(false);
        CountDownLatch latch = new CountDownLatch(1);
        activity.runOnUiThread(() -> {
            if (webView.canGoBack()) {
                webView.goBack();
                moved.set(true);
            }
            latch.countDown();
        });
        if (!latch.await(2, TimeUnit.SECONDS)) {
            throw new IllegalStateException("back dispatch timed out");
        }
        revision += 1;
        lastStateHash = "";
        domRuntimeInstalled = false;
        return new JSONObject()
            .put("ok", moved.get())
            .put("stale", false)
            .put("receipt", new JSONObject()
                .put("schemaVersion", "tempera.android.browser.action-receipt/v1")
                .put("kind", "back")
                .put("beforeStateHash", before.optString("documentStateHash", "")))
            .put("after", snapshot());
    }

    private void ensureDomRuntime() throws Exception {
        if (domRuntimeInstalled) {
            return;
        }
        JSONObject installed = evaluateObject(DomProgram.install());
        if (!installed.optBoolean("ok", false) || installed.optInt("version", 0) != 1) {
            throw new IllegalStateException("Tempera DOM runtime installation failed");
        }
        domRuntimeInstalled = true;
    }

    private JSONObject evaluateObject(String script) throws Exception {
        CountDownLatch latch = new CountDownLatch(1);
        AtomicReference<String> value = new AtomicReference<>();
        AtomicReference<Throwable> failure = new AtomicReference<>();
        activity.runOnUiThread(() -> {
            try {
                webView.evaluateJavascript(script, result -> {
                    value.set(result);
                    latch.countDown();
                });
            } catch (Throwable error) {
                failure.set(error);
                latch.countDown();
            }
        });
        if (!latch.await(EVALUATION_TIMEOUT_MS, TimeUnit.MILLISECONDS)) {
            throw new IllegalStateException("WebView JavaScript evaluation timed out");
        }
        if (failure.get() != null) {
            throw new IllegalStateException("WebView JavaScript evaluation failed", failure.get());
        }
        String callback = value.get();
        if (callback == null || "null".equals(callback)) {
            throw new IllegalStateException("WebView returned no DOM result");
        }
        Object outer = new JSONTokener(callback).nextValue();
        String json = outer instanceof String ? (String) outer : callback;
        return new JSONObject(json);
    }

    private JSONObject decorateSnapshot(JSONObject snapshot) throws JSONException {
        String hash = snapshot.optString("documentStateHash", "");
        if (!hash.equals(lastStateHash)) {
            revision += 1;
            lastStateHash = hash;
        }
        snapshot.put("revision", revision);
        snapshot.put("targetKind", "android-webview");
        snapshot.put("package", activity.getPackageName());
        lastUrl = snapshot.optString("url", lastUrl);
        return snapshot;
    }

    private void decorateNestedSnapshot(JSONObject result, String key) throws JSONException {
        JSONObject nested = result.optJSONObject(key);
        if (nested != null) {
            decorateSnapshot(nested);
        }
    }

    private boolean containsExactText(JSONObject snapshot, String exactText) {
        if (snapshot.optJSONArray("nodes") == null) {
            return false;
        }
        for (int index = 0; index < snapshot.optJSONArray("nodes").length(); index += 1) {
            JSONObject node = snapshot.optJSONArray("nodes").optJSONObject(index);
            if (node != null && exactText.equals(node.optString("label"))) {
                return true;
            }
        }
        return false;
    }
}
