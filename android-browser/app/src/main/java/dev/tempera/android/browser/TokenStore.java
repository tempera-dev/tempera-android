package dev.tempera.android.browser;

import android.content.Context;

import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.security.SecureRandom;

final class TokenStore {
    private static final String FILE_NAME = "control-token";
    private static final SecureRandom RANDOM = new SecureRandom();

    private TokenStore() {}

    static String loadOrCreate(Context context) throws IOException {
        File tokenFile = new File(context.getFilesDir(), FILE_NAME);
        if (tokenFile.isFile()) {
            try (FileInputStream input = new FileInputStream(tokenFile)) {
                byte[] bytes = new byte[(int) tokenFile.length()];
                int offset = 0;
                while (offset < bytes.length) {
                    int read = input.read(bytes, offset, bytes.length - offset);
                    if (read < 0) {
                        break;
                    }
                    offset += read;
                }
                String existing = new String(bytes, 0, offset, StandardCharsets.US_ASCII).trim();
                if (existing.matches("[0-9a-f]{64}")) {
                    return existing;
                }
            }
        }

        byte[] secret = new byte[32];
        RANDOM.nextBytes(secret);
        StringBuilder encoded = new StringBuilder(secret.length * 2);
        for (byte value : secret) {
            encoded.append(String.format("%02x", value & 0xff));
        }
        String token = encoded.toString();
        try (FileOutputStream output = context.openFileOutput(FILE_NAME, Context.MODE_PRIVATE)) {
            output.write(token.getBytes(StandardCharsets.US_ASCII));
            output.flush();
            output.getFD().sync();
        }
        return token;
    }
}
