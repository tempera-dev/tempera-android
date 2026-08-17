package dev.tempera.android.bridge;

import android.graphics.Rect;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;

import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

import java.util.List;

final class NodeCodec {
    static final int MAX_NODES = 600;
    static final int MAX_WINDOWS = 16;

    private NodeCodec() {}

    static JSONObject snapshot(BridgeAccessibilityService service) throws JSONException {
        JSONObject out = new JSONObject();
        out.put("revision", service.currentRevision());
        out.put("package", service.currentPackage());
        out.put("activity", service.currentClassName());
        out.put("screen", new JSONArray().put(service.screenWidth()).put(service.screenHeight()));

        JSONArray nodes = new JSONArray();
        JSONArray windowsJson = new JSONArray();
        int[] count = new int[]{0};
        List<AccessibilityWindowInfo> windows = service.getWindows();
        if (windows != null && !windows.isEmpty()) {
            int limit = Math.min(windows.size(), MAX_WINDOWS);
            for (int index = 0; index < limit && count[0] < MAX_NODES; index++) {
                AccessibilityWindowInfo window = windows.get(index);
                if (window == null) continue;
                AccessibilityNodeInfo root = null;
                try {
                    windowsJson.put(windowJson(window));
                    root = window.getRoot();
                    if (root != null) {
                        append(root, "w" + window.getId() + ":0", 0, nodes, count);
                    }
                } finally {
                    if (root != null) root.recycle();
                    window.recycle();
                }
            }
        } else {
            AccessibilityNodeInfo root = service.getRootInActiveWindow();
            if (root != null) {
                try {
                    append(root, "0", 0, nodes, count);
                } finally {
                    root.recycle();
                }
            }
        }
        out.put("windows", windowsJson);
        out.put("nodes", nodes);
        out.put("truncated", count[0] >= MAX_NODES);
        return out;
    }

    static AccessibilityNodeInfo findByRef(BridgeAccessibilityService service, String ref) {
        List<AccessibilityWindowInfo> windows = service.getWindows();
        if (windows != null && !windows.isEmpty()) {
            int limit = Math.min(windows.size(), MAX_WINDOWS);
            for (int index = 0; index < limit; index++) {
                AccessibilityWindowInfo window = windows.get(index);
                if (window == null) continue;
                AccessibilityNodeInfo root = null;
                boolean returnRoot = false;
                try {
                    root = window.getRoot();
                    if (root == null) continue;
                    AccessibilityNodeInfo found = find(root, "w" + window.getId() + ":0", ref);
                    if (found == null) continue;
                    if (found == root) {
                        returnRoot = true;
                        return root;
                    }
                    root.recycle();
                    root = null;
                    return found;
                } finally {
                    if (root != null && !returnRoot) root.recycle();
                    window.recycle();
                }
            }
            return null;
        }

        AccessibilityNodeInfo root = service.getRootInActiveWindow();
        if (root == null) return null;
        AccessibilityNodeInfo found = find(root, "0", ref);
        if (found == null || found != root) root.recycle();
        return found;
    }

    private static AccessibilityNodeInfo find(AccessibilityNodeInfo node, String path, String wanted) {
        if (stableRef(node, path).equals(wanted)) return node;
        int children = node.getChildCount();
        for (int index = 0; index < children; index++) {
            AccessibilityNodeInfo child = node.getChild(index);
            if (child == null) continue;
            AccessibilityNodeInfo found = find(child, path + "/" + index, wanted);
            if (found != null) {
                if (found != child) child.recycle();
                return found;
            }
            child.recycle();
        }
        return null;
    }

    private static void append(
            AccessibilityNodeInfo node,
            String path,
            int depth,
            JSONArray output,
            int[] count
    ) throws JSONException {
        if (count[0] >= MAX_NODES) return;
        Rect bounds = new Rect();
        node.getBoundsInScreen(bounds);
        if (!bounds.isEmpty()) {
            output.put(toJson(node, path, depth, bounds));
            count[0] += 1;
        }
        if (count[0] >= MAX_NODES) return;
        int children = node.getChildCount();
        for (int index = 0; index < children; index++) {
            AccessibilityNodeInfo child = node.getChild(index);
            if (child == null) continue;
            try {
                append(child, path + "/" + index, depth + 1, output, count);
            } finally {
                child.recycle();
            }
            if (count[0] >= MAX_NODES) break;
        }
    }

    private static JSONObject toJson(
            AccessibilityNodeInfo node,
            String path,
            int depth,
            Rect bounds
    ) throws JSONException {
        boolean password = node.isPassword();
        String text = password ? "" : chars(node.getText());
        String description = chars(node.getContentDescription());
        String hint = password ? "" : chars(node.getHintText());
        String viewId = text(node.getViewIdResourceName());
        String className = chars(node.getClassName());
        String packageName = chars(node.getPackageName());
        String label = firstNonEmpty(text, description, hint, shortId(viewId));

        JSONObject out = new JSONObject();
        out.put("ref", stableRef(node, path));
        out.put("label", label);
        out.put("class", shortClass(className));
        out.put("window_id", node.getWindowId());
        out.put("bounds", new JSONArray()
                .put(bounds.left).put(bounds.top).put(bounds.right).put(bounds.bottom));
        out.put("depth", depth);
        if (!text.isEmpty()) out.put("text", text);
        if (!description.isEmpty() && !description.equals(label)) out.put("desc", description);
        if (!hint.isEmpty() && !hint.equals(label)) out.put("hint", hint);
        if (!viewId.isEmpty()) out.put("id", viewId);
        if (!packageName.isEmpty()) out.put("package", packageName);
        if (node.isClickable()) out.put("clickable", true);
        if (node.isLongClickable()) out.put("long_clickable", true);
        if (node.isEditable()) out.put("editable", true);
        if (node.isScrollable()) out.put("scrollable", true);
        if (node.isSelected()) out.put("selected", true);
        if (node.isChecked()) out.put("checked", true);
        if (node.isFocused()) out.put("input_focused", true);
        if (password) out.put("password", true);
        if (!node.isEnabled()) out.put("enabled", false);
        return out;
    }

    static String stableRef(AccessibilityNodeInfo node, String path) {
        Rect bounds = new Rect();
        node.getBoundsInScreen(bounds);
        String viewId = text(node.getViewIdResourceName());
        String className = chars(node.getClassName());
        String identity = node.getWindowId() + "|"
                + (viewId.isEmpty() ? path : viewId) + "|"
                + className + "|"
                + bounds.left + "," + bounds.top + "," + bounds.right + "," + bounds.bottom;
        return "b" + Long.toUnsignedString(fnv1a64(identity), 36);
    }

    private static JSONObject windowJson(AccessibilityWindowInfo window) throws JSONException {
        JSONObject out = new JSONObject();
        out.put("id", window.getId());
        out.put("type", window.getType());
        out.put("layer", window.getLayer());
        out.put("active", window.isActive());
        out.put("focused", window.isFocused());
        CharSequence title = window.getTitle();
        if (title != null && title.length() > 0) out.put("title", title.toString());
        Rect bounds = new Rect();
        window.getBoundsInScreen(bounds);
        out.put("bounds", new JSONArray().put(bounds.left).put(bounds.top).put(bounds.right).put(bounds.bottom));
        return out;
    }

    private static long fnv1a64(String value) {
        long hash = 0xcbf29ce484222325L;
        byte[] bytes = value.getBytes(java.nio.charset.StandardCharsets.UTF_8);
        for (byte valueByte : bytes) {
            hash ^= (valueByte & 0xffL);
            hash *= 0x100000001b3L;
        }
        return hash;
    }

    private static String chars(CharSequence value) {
        return value == null ? "" : value.toString().trim();
    }

    private static String text(String value) {
        return value == null ? "" : value.trim();
    }

    private static String firstNonEmpty(String... values) {
        for (String value : values) {
            if (value != null && !value.isEmpty()) return value;
        }
        return "";
    }

    private static String shortId(String viewId) {
        int slash = viewId.lastIndexOf('/');
        return slash >= 0 ? viewId.substring(slash + 1) : viewId;
    }

    private static String shortClass(String className) {
        int dot = className.lastIndexOf('.');
        return dot >= 0 ? className.substring(dot + 1) : className;
    }
}
