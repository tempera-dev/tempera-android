package dev.tempera.android.bridge;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.BufferedWriter;
import java.io.InputStreamReader;
import java.io.OutputStreamWriter;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.LinkedHashMap;
import java.util.Map;
import java.util.UUID;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

final class BridgeServer extends Thread {
    static final int DEVICE_PORT = 6210;
    static final int PROTOCOL_VERSION = 3;
    static final int MAX_REQUEST_CHARS = 1_000_000;
    static final int MAX_ACTIONS = 12;
    static final int MAX_RESPONSE_CACHE = 256;

    private final BridgeAccessibilityService service;
    private final String epoch = UUID.randomUUID().toString();
    private final Object dispatchLock = new Object();
    private final ExecutorService clients = Executors.newFixedThreadPool(4);
    private final LinkedHashMap<String, String> responseCache = new LinkedHashMap<String, String>(64, 0.75f, true) {
        @Override
        protected boolean removeEldestEntry(Map.Entry<String, String> eldest) {
            return size() > MAX_RESPONSE_CACHE;
        }
    };
    private volatile boolean closed;
    private volatile ServerSocket listener;

    BridgeServer(BridgeAccessibilityService service) {
        super("tempera-android-bridge");
        setDaemon(true);
        this.service = service;
    }

    @Override
    public void run() {
        try (ServerSocket server = new ServerSocket()) {
            listener = server;
            server.setReuseAddress(true);
            server.bind(new InetSocketAddress(InetAddress.getLoopbackAddress(), DEVICE_PORT));
            while (!closed) {
                try {
                    Socket client = server.accept();
                    client.setTcpNoDelay(true);
                    client.setSoTimeout(120_000);
                    clients.execute(() -> handleClient(client));
                } catch (Exception ignored) {
                    if (closed) return;
                }
            }
        } catch (Exception ignored) {
            // Host health checks expose startup/bind failures without creating a public log surface.
        } finally {
            listener = null;
        }
    }

    void close() {
        closed = true;
        ServerSocket current = listener;
        if (current != null) {
            try {
                current.close();
            } catch (Exception ignored) {
            }
        }
        clients.shutdownNow();
        interrupt();
    }

    private void handleClient(Socket socket) {
        try (Socket client = socket;
             BufferedReader reader = new BufferedReader(new InputStreamReader(client.getInputStream(), StandardCharsets.UTF_8));
             BufferedWriter writer = new BufferedWriter(new OutputStreamWriter(client.getOutputStream(), StandardCharsets.UTF_8))) {
            String line;
            while (!closed && (line = reader.readLine()) != null) {
                JSONObject response;
                if (line.length() > MAX_REQUEST_CHARS) {
                    response = error(null, "request too large");
                } else {
                    try {
                        JSONObject request = new JSONObject(line);
                        synchronized (dispatchLock) {
                            response = dispatch(request);
                        }
                    } catch (Exception exc) {
                        response = error(null, safeMessage(exc));
                    }
                }
                writer.write(response.toString());
                writer.write('\n');
                writer.flush();
            }
        } catch (Exception ignored) {
        }
    }

    private JSONObject dispatch(JSONObject request) {
        Object requestId = request.opt("id");
        String expectedToken = service.readToken();
        String suppliedToken = request.optString("token", "");
        if (expectedToken.length() < 32 || !constantTimeEquals(expectedToken, suppliedToken)) {
            return error(requestId, "unauthorized");
        }

        String op = request.optString("op", "");
        if (!"health".equals(op)) {
            String suppliedEpoch = request.optString("server_epoch", "");
            if (!constantTimeEquals(epoch, suppliedEpoch)) {
                return error(requestId, "server epoch mismatch; reconnect and refresh bridge health");
            }
        }

        String cacheKey = cacheKey(request);
        if (cacheKey != null) {
            String cached = responseCache.get(cacheKey);
            if (cached != null) {
                try {
                    return new JSONObject(cached);
                } catch (Exception ignored) {
                    responseCache.remove(cacheKey);
                }
            }
        }

        JSONObject response;
        try {
            JSONObject result = execute(request, op);
            response = ok(requestId, result);
        } catch (Exception exc) {
            response = error(requestId, safeMessage(exc));
        }
        if (cacheKey != null) {
            responseCache.put(cacheKey, response.toString());
        }
        return response;
    }

    private JSONObject execute(JSONObject request, String op) throws Exception {
        JSONObject result = new JSONObject();
        switch (op) {
            case "health":
                result.put("service", "tempera-android-bridge");
                result.put("protocol", PROTOCOL_VERSION);
                result.put("server_epoch", epoch);
                result.put("revision", service.currentRevision());
                result.put("max_actions", MAX_ACTIONS);
                result.put("capabilities", new JSONArray()
                        .put("revision_guard")
                        .put("settled_act_observe")
                        .put("at_most_once_retry")
                        .put("multi_client_serial_dispatch")
                        .put("password_redaction"));
                break;
            case "observe":
                result = service.observe();
                break;
            case "act": {
                long expectedRevision = request.optLong("expected_revision", 0L);
                long actualRevision = service.currentRevision();
                if (expectedRevision > 0L && expectedRevision != actualRevision) {
                    return staleResult(expectedRevision, actualRevision);
                }
                JSONArray actions = actions(request);
                result.put("results", service.executeActions(actions));
                result.put("revision", service.currentRevision());
                break;
            }
            case "act_observe": {
                long expectedRevision = request.optLong("expected_revision", 0L);
                long actualRevision = service.currentRevision();
                if (expectedRevision > 0L && expectedRevision != actualRevision) {
                    return staleResult(expectedRevision, actualRevision);
                }
                JSONArray actions = actions(request);
                long before = service.currentRevision();
                JSONArray receipts = service.executeActions(actions);
                result.put("results", receipts);
                JSONObject transition;
                if (hasSuccessfulMutatingAction(receipts)) {
                    long firstChangeTimeoutMs = clamp(request.optLong("timeout_ms", 900L), 0L, 5_000L);
                    long quietMs = clamp(request.optLong("quiet_ms", 120L), 20L, 1_000L);
                    long maxSettleMs = clamp(request.optLong("max_settle_ms", 900L), quietMs, 5_000L);
                    transition = service.waitForSettledRevision(before, firstChangeTimeoutMs, quietMs, maxSettleMs);
                } else {
                    transition = service.transitionNow(before);
                }
                result.put("transition", transition);
                result.put("changed", transition.optBoolean("changed", false));
                result.put("settled", transition.optBoolean("settled", true));
                result.put("observation", service.observe());
                break;
            }
            case "wait_observe": {
                long after = request.optLong("after_revision", service.currentRevision());
                long firstChangeTimeoutMs = clamp(request.optLong("timeout_ms", 2_000L), 0L, 15_000L);
                long quietMs = clamp(request.optLong("quiet_ms", 120L), 20L, 1_000L);
                long maxSettleMs = clamp(request.optLong("max_settle_ms", 900L), quietMs, 5_000L);
                JSONObject transition = service.waitForSettledRevision(after, firstChangeTimeoutMs, quietMs, maxSettleMs);
                result.put("transition", transition);
                result.put("changed", transition.optBoolean("changed", false));
                result.put("settled", transition.optBoolean("settled", true));
                result.put("observation", service.observe());
                break;
            }
            case "screenshot":
                result.put("png_base64", service.screenshotBase64());
                result.put("revision", service.currentRevision());
                break;
            default:
                throw new IllegalArgumentException("unsupported operation: " + op);
        }
        return result;
    }

    private JSONObject staleResult(long expectedRevision, long actualRevision) throws Exception {
        JSONObject result = new JSONObject();
        result.put("stale", true);
        result.put("expected_revision", expectedRevision);
        result.put("revision", actualRevision);
        result.put("observation", service.observe());
        return result;
    }

    private static JSONArray actions(JSONObject request) {
        JSONArray actions = request.optJSONArray("actions");
        if (actions == null) throw new IllegalArgumentException("actions must be an array");
        if (actions.length() == 0) throw new IllegalArgumentException("actions must not be empty");
        if (actions.length() > MAX_ACTIONS) {
            throw new IllegalArgumentException("action batch exceeds device limit of " + MAX_ACTIONS);
        }
        return actions;
    }

    private static boolean hasSuccessfulMutatingAction(JSONArray receipts) {
        for (int index = 0; index < receipts.length(); index++) {
            JSONObject receipt = receipts.optJSONObject(index);
            if (receipt == null || !receipt.optBoolean("ok", false)) continue;
            JSONObject action = receipt.optJSONObject("action");
            if (action == null) continue;
            if (!"wait".equals(action.optString("type", ""))) return true;
        }
        return false;
    }

    private String cacheKey(JSONObject request) {
        Object id = request.opt("id");
        if (id == null || JSONObject.NULL.equals(id)) return null;
        String clientId = request.optString("client_id", "");
        if (clientId.length() < 16 || clientId.length() > 128) return null;
        return clientId + ":" + String.valueOf(id);
    }

    private static long clamp(long value, long low, long high) {
        return Math.max(low, Math.min(high, value));
    }

    private static JSONObject ok(Object id, JSONObject result) throws Exception {
        JSONObject response = new JSONObject();
        if (id != null) response.put("id", id);
        response.put("ok", true);
        response.put("result", result);
        return response;
    }

    private static JSONObject error(Object id, String message) {
        JSONObject response = new JSONObject();
        try {
            if (id != null) response.put("id", id);
            response.put("ok", false);
            response.put("error", message);
        } catch (Exception ignored) {
        }
        return response;
    }

    private static boolean constantTimeEquals(String expected, String actual) {
        return MessageDigest.isEqual(
                expected.getBytes(StandardCharsets.UTF_8),
                actual.getBytes(StandardCharsets.UTF_8));
    }

    private static String safeMessage(Exception exception) {
        String message = exception.getMessage();
        if (message == null || message.isEmpty()) return exception.getClass().getSimpleName();
        return message.length() > 500 ? message.substring(0, 500) : message;
    }
}
