package dev.tempera.android.browser;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.Bundle;
import android.view.Gravity;
import android.view.ViewGroup;
import android.webkit.CookieManager;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.Toast;

import java.io.IOException;

public final class TemperaBrowserActivity extends Activity {
    private WebView webView;
    private EditText address;
    private ProgressBar progress;
    private BrowserRuntime runtime;
    private ControlServer controlServer;

    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        WebView.setWebContentsDebuggingEnabled(false);
        setContentView(buildUi());
        configureWebView();
        runtime = new BrowserRuntime(this, webView);
        try {
            controlServer = new ControlServer(runtime, TokenStore.loadOrCreate(this));
            controlServer.start();
        } catch (IOException error) {
            Toast.makeText(this, "Tempera browser control failed to start", Toast.LENGTH_LONG).show();
        }
        navigateFromIntent(getIntent(), savedInstanceState == null);
    }

    private LinearLayout buildUi() {
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setLayoutParams(new ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));

        LinearLayout toolbar = new LinearLayout(this);
        toolbar.setOrientation(LinearLayout.HORIZONTAL);
        toolbar.setGravity(Gravity.CENTER_VERTICAL);
        int padding = Math.round(8 * getResources().getDisplayMetrics().density);
        toolbar.setPadding(padding, padding, padding, padding);

        address = new EditText(this);
        address.setSingleLine(true);
        address.setHint("https://example.com");
        address.setInputType(android.text.InputType.TYPE_CLASS_TEXT
            | android.text.InputType.TYPE_TEXT_VARIATION_URI);
        toolbar.addView(address, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));

        Button go = new Button(this);
        go.setText("Go");
        go.setOnClickListener(view -> navigate(address.getText().toString()));
        toolbar.addView(go, new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.WRAP_CONTENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ));
        root.addView(toolbar, new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ));

        progress = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        progress.setMax(100);
        progress.setVisibility(ProgressBar.GONE);
        root.addView(progress, new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            Math.max(2, Math.round(2 * getResources().getDisplayMetrics().density))
        ));

        webView = new WebView(this);
        root.addView(webView, new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            0,
            1
        ));
        return root;
    }

    private void configureWebView() {
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true);
        settings.setDomStorageEnabled(true);
        settings.setDatabaseEnabled(false);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(false);
        settings.setJavaScriptCanOpenWindowsAutomatically(false);
        settings.setSupportMultipleWindows(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setSafeBrowsingEnabled(true);
        settings.setMediaPlaybackRequiresUserGesture(true);
        settings.setCacheMode(WebSettings.LOAD_DEFAULT);
        settings.setBuiltInZoomControls(false);
        settings.setDisplayZoomControls(false);
        settings.setUserAgentString(settings.getUserAgentString() + " TemperaAndroidBrowser/0.1");

        CookieManager cookies = CookieManager.getInstance();
        cookies.setAcceptCookie(true);
        cookies.setAcceptThirdPartyCookies(webView, false);

        webView.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                Uri uri = request.getUrl();
                return !"https".equalsIgnoreCase(uri.getScheme());
            }

            @Override
            public void onPageStarted(WebView view, String url, Bitmap favicon) {
                if (runtime != null) {
                    runtime.onPageStarted(url);
                }
                address.setText(url);
                progress.setVisibility(ProgressBar.VISIBLE);
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                if (runtime != null) {
                    runtime.onPageFinished(url);
                }
                address.setText(url);
                progress.setVisibility(ProgressBar.GONE);
            }
        });
        webView.setWebChromeClient(new WebChromeClient() {
            @Override
            public void onProgressChanged(WebView view, int newProgress) {
                progress.setProgress(newProgress);
                progress.setVisibility(newProgress >= 100 ? ProgressBar.GONE : ProgressBar.VISIBLE);
            }
        });
    }

    private void navigateFromIntent(Intent intent, boolean initial) {
        Uri data = intent == null ? null : intent.getData();
        if (data != null && "https".equalsIgnoreCase(data.getScheme())) {
            navigate(data.toString());
        } else if (initial) {
            navigate("about:blank");
        }
    }

    private void navigate(String raw) {
        String url = raw == null ? "" : raw.trim();
        if (url.isEmpty()) {
            return;
        }
        if (!url.contains("://") && !"about:blank".equals(url)) {
            url = "https://" + url;
        }
        Uri uri = Uri.parse(url);
        if (!("https".equalsIgnoreCase(uri.getScheme()) || "about:blank".equals(url))) {
            Toast.makeText(this, "Only HTTPS navigation is enabled", Toast.LENGTH_SHORT).show();
            return;
        }
        webView.loadUrl(url);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        navigateFromIntent(intent, false);
    }

    @Override
    public void onBackPressed() {
        if (webView != null && webView.canGoBack()) {
            webView.goBack();
        } else {
            super.onBackPressed();
        }
    }

    @Override
    protected void onDestroy() {
        if (controlServer != null) {
            controlServer.close();
        }
        if (webView != null) {
            webView.stopLoading();
            webView.clearHistory();
            webView.removeAllViews();
            webView.destroy();
        }
        super.onDestroy();
    }
}
