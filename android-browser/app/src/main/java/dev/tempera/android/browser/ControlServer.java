package dev.tempera.android.browser;

import org.json.JSONObject;

import java.io.BufferedInputStream;
import java.io.BufferedOutputStream;
import java.io.ByteArrayOutputStream;
import java.io.Closeable;
import java.io.IOException;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;

final class ControlServer implements Closeable {
    static final int PORT = 7433;
    private static final int MAX_HEADER_LINE = 8 * 1024;
    private static final int MAX_HEADERS = 64;
    private static final int MAX_BODY = 1024 * 1024;
    private static final int MAX_REQUESTS_PER_CONNECTION = 256;
    private static final int CONNECTION_IDLE_TIMEOUT_MS = 15_000;

    private final BrowserRuntime runtime;
    private final String bearerToken;
    private final AtomicBoolean running = new AtomicBoolean(false);
    private final ExecutorService clients = Executors.newFixedThreadPool(4, runnable -> {
        Thread thread = new Thread(runnable, "tempera-browser-control-client");
        thread.setDaemon(true);
        return thread;
    });
    private ServerSocket server;
    private Thread acceptThread;

    ControlServer(BrowserRuntime runtime, String bearerToken) {
        this.runtime = runtime;
        this.bearerToken = bearerToken;
    }

    void start() throws IOException {
        if (!running.compareAndSet(false, true)) {
            return;
        }
        server = new ServerSocket();
        server.setReuseAddress(true);
        // adb's tcp forward targets the device IPv4 loopback endpoint. Avoid
        // InetAddress.getLoopbackAddress(), which may select ::1 on Android
        // and leave the forwarded IPv4 connection with no listening peer.
        server.bind(
            new InetSocketAddress(InetAddress.getByAddress(new byte[]{127, 0, 0, 1}), PORT),
            16
        );
        acceptThread = new Thread(this::acceptLoop, "tempera-browser-control-accept");
        acceptThread.setDaemon(true);
        acceptThread.start();
    }

    private void acceptLoop() {
        while (running.get()) {
            try {
                Socket socket = server.accept();
                socket.setTcpNoDelay(true);
                socket.setKeepAlive(true);
                socket.setSoTimeout(CONNECTION_IDLE_TIMEOUT_MS);
                clients.execute(() -> handle(socket));
            } catch (IOException error) {
                if (running.get()) {
                    android.util.Log.e("TemperaBrowser", "control accept failed", error);
                }
            }
        }
    }

    private void handle(Socket socket) {
        try (socket;
             BufferedInputStream input = new BufferedInputStream(socket.getInputStream());
             BufferedOutputStream output = new BufferedOutputStream(socket.getOutputStream())) {
            for (int requestIndex = 0;
                 requestIndex < MAX_REQUESTS_PER_CONNECTION && running.get();
                 requestIndex += 1) {
                final Request request;
                try {
                    request = readRequest(input);
                } catch (IllegalArgumentException error) {
                    write(output, 400, error(error.getMessage()), false);
                    return;
                } catch (IOException error) {
                    // Normal peer close or idle expiry. The request is either absent or
                    // incomplete, so never attempt to replay or synthesize a mutation.
                    return;
                }

                if (!constantTimeEquals("Bearer " + bearerToken, request.headers.get("authorization"))) {
                    write(output, 401, error("unauthorized"), false);
                    return;
                }

                boolean requestedClose = "close".equalsIgnoreCase(request.headers.get("connection"));
                boolean keepAlive = !requestedClose
                    && requestIndex + 1 < MAX_REQUESTS_PER_CONNECTION;
                final JSONObject response;
                try {
                    response = route(request);
                } catch (IllegalArgumentException error) {
                    write(output, 400, error(error.getMessage()), false);
                    return;
                } catch (Exception error) {
                    android.util.Log.e("TemperaBrowser", "control request failed", error);
                    write(output, 500, error("browser control request failed"), false);
                    return;
                }
                write(output, 200, response, keepAlive);
                if (!keepAlive) {
                    return;
                }
            }
        } catch (IOException error) {
            if (running.get()) {
                android.util.Log.d("TemperaBrowser", "control connection ended", error);
            }
        }
    }

    private JSONObject route(Request request) throws Exception {
        if ("GET".equals(request.method) && "/v1/health".equals(request.path)) {
            return runtime.health();
        }
        if ("GET".equals(request.method) && "/v1/snapshot".equals(request.path)) {
            return runtime.snapshot();
        }
        if (!"POST".equals(request.method)) {
            throw new IllegalArgumentException("unsupported method or path");
        }
        JSONObject body = request.body.length == 0
            ? new JSONObject()
            : new JSONObject(new String(request.body, StandardCharsets.UTF_8));
        return switch (request.path) {
            case "/v1/navigate" -> runtime.navigate(body.optString("url", ""));
            case "/v1/snapshot-delta" -> runtime.snapshotDelta(body);
            case "/v1/action" -> runtime.action(body);
            case "/v1/act-observe" -> runtime.actObserve(body);
            case "/v1/wait" -> runtime.waitFor(body);
            default -> throw new IllegalArgumentException("unsupported method or path");
        };
    }

    private Request readRequest(BufferedInputStream input) throws IOException {
        String requestLine = readLine(input);
        String[] parts = requestLine.split(" ", 3);
        if (parts.length != 3 || !parts[2].startsWith("HTTP/1.")) {
            throw new IllegalArgumentException("invalid HTTP request line");
        }
        String method = parts[0].toUpperCase(Locale.ROOT);
        String path = parts[1].split("\\?", 2)[0];
        Map<String, String> headers = new HashMap<>();
        for (int count = 0; count < MAX_HEADERS; count += 1) {
            String line = readLine(input);
            if (line.isEmpty()) {
                break;
            }
            int separator = line.indexOf(':');
            if (separator <= 0) {
                throw new IllegalArgumentException("invalid HTTP header");
            }
            headers.put(
                line.substring(0, separator).trim().toLowerCase(Locale.ROOT),
                line.substring(separator + 1).trim()
            );
            if (count == MAX_HEADERS - 1) {
                throw new IllegalArgumentException("too many HTTP headers");
            }
        }
        int contentLength = 0;
        if (headers.containsKey("content-length")) {
            try {
                contentLength = Integer.parseInt(headers.get("content-length"));
            } catch (NumberFormatException error) {
                throw new IllegalArgumentException("invalid Content-Length");
            }
        }
        if (contentLength < 0 || contentLength > MAX_BODY) {
            throw new IllegalArgumentException("request body exceeds limit");
        }
        byte[] body = readExactBody(input, contentLength);
        return new Request(method, path, headers, body);
    }

    private byte[] readExactBody(BufferedInputStream input, int length) throws IOException {
        byte[] body = new byte[length];
        int offset = 0;
        while (offset < length) {
            int read = input.read(body, offset, length - offset);
            if (read < 0) {
                throw new IllegalArgumentException("truncated request body");
            }
            if (read == 0) {
                continue;
            }
            offset += read;
        }
        return body;
    }

    private String readLine(BufferedInputStream input) throws IOException {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        while (bytes.size() <= MAX_HEADER_LINE) {
            int value = input.read();
            if (value < 0) {
                throw new IOException("unexpected end of stream");
            }
            if (value == '\n') {
                byte[] line = bytes.toByteArray();
                int length = line.length;
                if (length > 0 && line[length - 1] == '\r') {
                    length -= 1;
                }
                return new String(line, 0, length, StandardCharsets.US_ASCII);
            }
            bytes.write(value);
        }
        throw new IllegalArgumentException("HTTP header line exceeds limit");
    }

    private void write(OutputStream output, int status, JSONObject body, boolean keepAlive)
        throws IOException {
        byte[] payload = body.toString().getBytes(StandardCharsets.UTF_8);
        String reason = switch (status) {
            case 200 -> "OK";
            case 400 -> "Bad Request";
            case 401 -> "Unauthorized";
            default -> "Internal Server Error";
        };
        String connection = keepAlive ? "keep-alive" : "close";
        String keepAliveHeader = keepAlive
            ? "Keep-Alive: timeout=15, max=" + MAX_REQUESTS_PER_CONNECTION + "\r\n"
            : "";
        String headers = "HTTP/1.1 " + status + " " + reason + "\r\n"
            + "Content-Type: application/json\r\n"
            + "Content-Length: " + payload.length + "\r\n"
            + "Cache-Control: no-store\r\n"
            + "Connection: " + connection + "\r\n"
            + keepAliveHeader
            + "\r\n";
        output.write(headers.getBytes(StandardCharsets.US_ASCII));
        output.write(payload);
        output.flush();
    }

    private static JSONObject error(String message) {
        try {
            return new JSONObject()
                .put("schemaVersion", "tempera.android.browser.error/v1")
                .put("ok", false)
                .put("error", message == null ? "invalid request" : message);
        } catch (Exception impossible) {
            throw new IllegalStateException(impossible);
        }
    }

    private static boolean constantTimeEquals(String expected, String actual) {
        if (actual == null) {
            return false;
        }
        byte[] left = expected.getBytes(StandardCharsets.UTF_8);
        byte[] right = actual.getBytes(StandardCharsets.UTF_8);
        int difference = left.length ^ right.length;
        int length = Math.max(left.length, right.length);
        for (int index = 0; index < length; index += 1) {
            byte a = index < left.length ? left[index] : 0;
            byte b = index < right.length ? right[index] : 0;
            difference |= a ^ b;
        }
        return difference == 0;
    }

    @Override
    public void close() {
        running.set(false);
        try {
            if (server != null) {
                server.close();
            }
        } catch (IOException ignored) {
            // Best-effort shutdown during Activity destruction.
        }
        clients.shutdownNow();
    }

    private record Request(String method, String path, Map<String, String> headers, byte[] body) {}
}
