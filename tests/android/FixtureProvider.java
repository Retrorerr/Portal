package app.polarbear.files.tests;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import java.io.FileNotFoundException;
import java.io.FileOutputStream;

/** Generates public test bytes only; never exposes the test app's filesystem. */
public class FixtureProvider extends ContentProvider {
    public static final int LENGTH = 256 * 1024 * 17;
    @Override public boolean onCreate() { return true; }
    private void validate(Uri uri) throws FileNotFoundException {
        if (uri.getPathSegments().size() != 1 || !uri.getLastPathSegment().matches("portal-transfer-qa-[a-f0-9-]+-(uri|out|broken).bin")) throw new FileNotFoundException();
    }
    @Override public String getType(Uri uri) { return "application/octet-stream"; }
    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] selectionArgs, String sortOrder) {
        String[] columns = projection == null ? new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE} : projection;
        MatrixCursor cursor = new MatrixCursor(columns);
        Object[] values = new Object[columns.length];
        for (int i = 0; i < columns.length; i++) {
            if (columns[i].equals(OpenableColumns.DISPLAY_NAME)) values[i] = uri.getLastPathSegment();
            if (columns[i].equals(OpenableColumns.SIZE)) values[i] = uri.getLastPathSegment().endsWith("-broken.bin") ? null : LENGTH;
        }
        cursor.addRow(values); return cursor;
    }
    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        validate(uri);
        try {
            if (uri.getLastPathSegment().endsWith("-out.bin")) {
                java.io.File file = new java.io.File(getContext().getCacheDir(), uri.getLastPathSegment());
                if (mode.equals("r")) return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY);
                if (mode.equals("w")) return ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_CREATE | ParcelFileDescriptor.MODE_TRUNCATE | ParcelFileDescriptor.MODE_WRITE_ONLY);
            }
            if (!mode.equals("r")) throw new FileNotFoundException("Read only");
            ParcelFileDescriptor[] pipe = ParcelFileDescriptor.createReliablePipe();
            new Thread(() -> {
                try (FileOutputStream output = new ParcelFileDescriptor.AutoCloseOutputStream(pipe[1])) {
                    if (uri.getLastPathSegment().endsWith("-broken.bin")) {
                        output.write(new byte[] {1, 2, 3});
                        pipe[1].closeWithError("Provider stopped before finishing");
                        return;
                    }
                    byte[] buffer = new byte[256 * 1024];
                    for (int i = 0; i < buffer.length; i++) buffer[i] = (byte) i;
                    for (int i = 0; i < 17; i++) output.write(buffer);
                } catch (Exception error) { try { pipe[1].closeWithError(error.toString()); } catch (Exception ignored) {} }
            }, "fixture-writer").start();
            return pipe[0];
        } catch (Exception error) { throw new FileNotFoundException(error.toString()); }
    }
    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String selection, String[] args) {
        try {
            validate(uri);
            if (uri.getLastPathSegment().endsWith("-out.bin")) return new java.io.File(getContext().getCacheDir(), uri.getLastPathSegment()).delete() ? 1 : 0;
        } catch (FileNotFoundException ignored) {}
        return 0;
    }
}
