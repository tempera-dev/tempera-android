package dev.tempera.android.bridge;

import android.app.Activity;
import android.app.AlertDialog;
import android.graphics.Typeface;
import android.os.Bundle;
import android.text.InputType;
import android.view.View;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

/**
 * Frozen, offline UI fixture for measuring Android-agent control quality.
 *
 * This activity intentionally has no launcher intent. The trusted eval harness starts it by
 * explicit component. It contains no network calls, credentials, hidden state, or privileged
 * behavior; its purpose is to make regression measurements independent of Android Settings UI
 * wording changes while still exercising real Android widgets and Accessibility semantics.
 */
public final class EvalFixtureActivity extends Activity {
    private static final int PAD = 32;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        showRoot();
    }

    private LinearLayout column(String title) {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setPadding(PAD, PAD, PAD, PAD);
        TextView heading = new TextView(this);
        heading.setText(title);
        heading.setTextSize(26f);
        heading.setTypeface(Typeface.DEFAULT_BOLD);
        heading.setContentDescription(title);
        layout.addView(heading, matchWrap());
        return layout;
    }

    private LinearLayout.LayoutParams matchWrap() {
        return new LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.WRAP_CONTENT
        );
    }

    private Button button(String label, View.OnClickListener listener) {
        Button button = new Button(this);
        button.setText(label);
        button.setContentDescription(label);
        button.setOnClickListener(listener);
        return button;
    }

    private void setPage(View view) {
        setContentView(view);
    }

    private void showRoot() {
        LinearLayout page = column("Tempera Android Eval");
        page.addView(button("Wi-Fi lab", ignored -> showWifi()), matchWrap());
        page.addView(button("Profile lab", ignored -> showProfile()), matchWrap());
        page.addView(button("Dialog lab", ignored -> showDialogLab()), matchWrap());
        page.addView(button("Long list", ignored -> showLongList()), matchWrap());
        setPage(page);
    }

    private void showWifi() {
        LinearLayout page = column("Wi-Fi lab");
        TextView status = new TextView(this);
        status.setText("Status: connected");
        status.setContentDescription("Status: connected");
        page.addView(status, matchWrap());
        page.addView(button("Advanced", ignored -> showWifiAdvanced()), matchWrap());
        page.addView(button("Back to eval home", ignored -> showRoot()), matchWrap());
        setPage(page);
    }

    private void showWifiAdvanced() {
        LinearLayout page = column("Wi-Fi advanced");
        TextView proxy = new TextView(this);
        proxy.setText("Proxy mode: None");
        proxy.setContentDescription("Proxy mode: None");
        page.addView(proxy, matchWrap());
        TextView dns = new TextView(this);
        dns.setText("Private DNS: Automatic");
        dns.setContentDescription("Private DNS: Automatic");
        page.addView(dns, matchWrap());
        page.addView(button("Finish Wi-Fi lab", ignored -> showTerminal("Wi-Fi lab complete")), matchWrap());
        setPage(page);
    }

    private void showProfile() {
        LinearLayout page = column("Profile lab");
        EditText name = new EditText(this);
        name.setHint("Display name");
        name.setContentDescription("Display name");
        name.setSingleLine(true);
        name.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_WORDS);
        page.addView(name, matchWrap());

        EditText note = new EditText(this);
        note.setHint("Short note");
        note.setContentDescription("Short note");
        note.setSingleLine(true);
        note.setInputType(InputType.TYPE_CLASS_TEXT);
        page.addView(note, matchWrap());

        Button save = button("Save profile", ignored -> {
            int nameLength = name.getText() == null ? 0 : name.getText().length();
            int noteLength = note.getText() == null ? 0 : note.getText().length();
            LinearLayout terminal = column("Profile saved");
            TextView lengths = new TextView(this);
            lengths.setText("Lengths: name=" + nameLength + ", note=" + noteLength);
            lengths.setContentDescription("Lengths: name=" + nameLength + ", note=" + noteLength);
            terminal.addView(lengths, matchWrap());
            setPage(terminal);
        });
        page.addView(save, matchWrap());
        setPage(page);
    }

    private void showDialogLab() {
        LinearLayout page = column("Dialog lab");
        page.addView(button("Open permission dialog", ignored -> new AlertDialog.Builder(this)
                .setTitle("Permission simulation")
                .setMessage("Choose the one-time option for this synthetic fixture.")
                .setPositiveButton("Allow once", (dialog, which) -> showTerminal("Dialog accepted"))
                .setNegativeButton("Cancel", null)
                .show()), matchWrap());
        setPage(page);
    }

    private void showLongList() {
        LinearLayout content = column("Long list");
        for (int index = 1; index <= 40; index++) {
            final int item = index;
            String label = index == 40 ? "Target 40" : "Item " + index;
            content.addView(button(label, ignored -> {
                if (item == 40) showTerminal("Long list complete");
            }), matchWrap());
        }
        ScrollView scroll = new ScrollView(this);
        scroll.setContentDescription("Long list scroll area");
        scroll.addView(content, matchWrap());
        setPage(scroll);
    }

    private void showTerminal(String title) {
        LinearLayout page = column(title);
        TextView done = new TextView(this);
        done.setText("Fixture state: complete");
        done.setContentDescription("Fixture state: complete");
        page.addView(done, matchWrap());
        page.addView(button("Back to eval home", ignored -> showRoot()), matchWrap());
        setPage(page);
    }
}
