package app.polarbear.files.tests;

import android.content.ContentProvider;
import android.content.ContentValues;
import android.database.Cursor;
import android.database.MatrixCursor;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.provider.DocumentsContract;
import java.io.File;
import java.io.FileNotFoundException;

/** Implements the document IPC contract for generated batch-test files only. */
public final class SaveFixtureProvider extends ContentProvider {
    @Override public boolean onCreate() { return true; }
    private File document(String id) throws FileNotFoundException {
        if (!id.startsWith("portal-transfer-qa-") || id.contains("/") || id.contains("\\") || id.contains("..")) throw new FileNotFoundException();
        return new File(getContext().getCacheDir(), id);
    }
    @Override public Bundle call(String method, String arg, Bundle extras) {
        Uri uri = extras.getParcelable("uri");
        try {
            if (method.equals("android:createDocument")) {
                if (!DocumentsContract.getDocumentId(uri).equals("root")) throw new FileNotFoundException();
                String name = extras.getString(DocumentsContract.Document.COLUMN_DISPLAY_NAME);
                if (!document(name).createNewFile()) throw new FileNotFoundException("Duplicate fixture name");
                Bundle result = new Bundle();
                result.putParcelable("uri", DocumentsContract.buildDocumentUriUsingTree(uri, name));
                return result;
            }
            if (method.equals("android:deleteDocument")) {
                document(DocumentsContract.getDocumentId(uri)).delete();
                return new Bundle();
            }
        } catch (Exception error) { throw new IllegalStateException(error); }
        return super.call(method, arg, extras);
    }
    @Override public Cursor query(Uri uri, String[] projection, String selection, String[] args, String sortOrder) {
        return new MatrixCursor(new String[] { DocumentsContract.Document.COLUMN_DOCUMENT_ID });
    }
    @Override public String getType(Uri uri) { return "application/octet-stream"; }
    @Override public ParcelFileDescriptor openFile(Uri uri, String mode) throws FileNotFoundException {
        return ParcelFileDescriptor.open(document(DocumentsContract.getDocumentId(uri)), ParcelFileDescriptor.parseMode(mode));
    }
    @Override public Uri insert(Uri uri, ContentValues values) { throw new UnsupportedOperationException(); }
    @Override public int update(Uri uri, ContentValues values, String selection, String[] args) { throw new UnsupportedOperationException(); }
    @Override public int delete(Uri uri, String selection, String[] args) { throw new UnsupportedOperationException(); }
}
