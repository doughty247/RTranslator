/*
 * Copyright 2016 Luca Martino.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copyFile of the License at
 *
 * http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package nie.translator.rtranslator.tools.tts;

import android.app.DownloadManager;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.net.Uri;

import org.apache.commons.compress.archivers.tar.TarArchiveEntry;
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream;
import org.apache.commons.compress.compressors.bzip2.BZip2CompressorInputStream;

import java.io.BufferedInputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/**
 * Downloads and extracts one {@link NeuralTtsManager.VoiceEntry}'s voice model. Deliberately
 * separate from {@code access/Downloader.java} rather than extending it -- that class's
 * {@code DOWNLOAD_URLS}/{@code DOWNLOAD_NAMES}/{@code DOWNLOAD_SIZES} arrays are iterated
 * strictly by index with no per-file skip/opt-out concept (confirmed by reading
 * {@code DownloadReceiver.internalCheckAndStartNextDownload}), which is the right shape
 * for the mandatory first-run Whisper/NLLB download but the wrong shape for an optional,
 * user-triggered, one-language-at-a-time download -- bolting that on risked destabilizing
 * the app's critical first-run flow for a much lower-stakes feature. This class is
 * intentionally simpler: one archive at a time, no cross-file progress bookkeeping.
 * <p>
 * Unlike the mandatory Whisper/NLLB flow's flat model files, sherpa-onnx only distributes
 * its pretrained voices as a single {@code .tar.bz2} archive per voice (confirmed against
 * every "Download the model" section in k2-fsa/sherpa's
 * docs/source/onnx/tts/pretrained_models/vits.rst -- there is no flat-file alternative), so
 * this class downloads that one archive and extracts it directly, rather than downloading
 * independent model/tokens files the way the placeholder version of this class used to.
 * Downloads land in external app storage first (the only place {@link DownloadManager} can
 * target directly), get extracted into internal storage, and the external copy is then
 * discarded -- matching {@code DownloadFragment}'s own external -> internal staging shape,
 * minus the intermediate {@code FileTools#moveFile} step since extraction already produces
 * the final files directly in internal storage.
 */
public class VoiceModelDownloader {
    // Every voice in NeuralTtsManager.VOICE_CATALOG is hosted under this single upstream
    // release tag -- confirmed by k2-fsa/sherpa's own docs (every per-voice "Download the
    // model" section uses this exact base URL, e.g.
    // ".../releases/download/tts-models/vits-piper-en_US-lessac-medium.tar.bz2").
    private static final String BASE_URL = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/";

    public interface Listener {
        void onProgress(long downloadedBytes, long totalBytes);
        void onSuccess();
        void onFailure();
    }

    private final Context context;
    private BroadcastReceiver receiver;

    public VoiceModelDownloader(Context context) {
        this.context = context.getApplicationContext();
    }

    public void download(NeuralTtsManager.VoiceEntry entry, Listener listener) {
        DownloadManager downloadManager = (DownloadManager) context.getSystemService(Context.DOWNLOAD_SERVICE);
        String fileName = entry.archiveFileName;
        Uri uri = Uri.parse(BASE_URL + fileName);
        DownloadManager.Request request = new DownloadManager.Request(uri)
                .setDestinationInExternalFilesDir(context, null, fileName)
                .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_ONLY_COMPLETION)
                .setAllowedOverMetered(true);
        long downloadId = downloadManager.enqueue(request);
        pollProgress(downloadManager, downloadId, listener);

        receiver = new BroadcastReceiver() {
            @Override
            public void onReceive(Context ctx, Intent intent) {
                long completedId = intent.getLongExtra(DownloadManager.EXTRA_DOWNLOAD_ID, -1);
                if (completedId != downloadId) {
                    return;
                }
                context.unregisterReceiver(this);

                File archiveFile = new File(context.getExternalFilesDir(null), fileName);
                if (!archiveFile.exists()) {
                    listener.onFailure();
                    return;
                }
                File targetDir = new File(new File(context.getFilesDir(), "tts_models"), entry.languageCode);
                if (!targetDir.exists() && !targetDir.mkdirs()) {
                    archiveFile.delete();
                    listener.onFailure();
                    return;
                }
                boolean extracted = extractArchive(archiveFile, targetDir);
                archiveFile.delete();
                if (extracted) {
                    listener.onSuccess();
                } else {
                    listener.onFailure();
                }
            }
        };
        context.registerReceiver(receiver, new IntentFilter(DownloadManager.ACTION_DOWNLOAD_COMPLETE));
    }

    /**
     * Extracts a sherpa-onnx voice {@code .tar.bz2} into {@code targetDir}, flattening two
     * levels of the archive's own layout to match what {@link NeuralTtsManager#create}
     * expects on disk:
     * <ul>
     *     <li>the archive's single top-level folder (e.g.
     *     {@code vits-piper-en_US-lessac-medium/}) is dropped -- its contents land directly
     *     in {@code targetDir}</li>
     *     <li>within that, an {@code espeak-ng-data/} folder (present in every Piper voice's
     *     archive, per docs/source/onnx/tts/piper.rst) has its own contents flattened
     *     directly into {@code targetDir} too, rather than kept in a subfolder -- VITS's
     *     {@code dataDir} config parameter must point directly at the directory containing
     *     espeak-ng-data's files, and {@code NeuralTtsManager} already passes
     *     {@code targetDir} itself as that {@code dataDir}</li>
     * </ul>
     * Guards against zip-slip (archive entries that try to write outside {@code targetDir}
     * via {@code ../} path segments) by resolving every entry's path against
     * {@code targetDir} and rejecting anything that escapes it.
     */
    private boolean extractArchive(File archiveFile, File targetDir) {
        try (InputStream fileIn = new FileInputStream(archiveFile);
             BufferedInputStream buffered = new BufferedInputStream(fileIn);
             BZip2CompressorInputStream bzip2In = new BZip2CompressorInputStream(buffered);
             TarArchiveInputStream tarIn = new TarArchiveInputStream(bzip2In)) {

            String targetCanonicalPath = targetDir.getCanonicalPath();
            TarArchiveEntry entry;
            while ((entry = tarIn.getNextTarEntry()) != null) {
                if (entry.isDirectory()) {
                    continue;
                }
                String relativePath = stripArchiveLayout(entry.getName());
                if (relativePath == null || relativePath.isEmpty()) {
                    continue;
                }
                File outFile = new File(targetDir, relativePath);
                if (!outFile.getCanonicalPath().startsWith(targetCanonicalPath + File.separator)) {
                    return false; // entry tried to escape targetDir
                }
                File parent = outFile.getParentFile();
                if (parent != null && !parent.exists() && !parent.mkdirs()) {
                    return false;
                }
                try (OutputStream out = new FileOutputStream(outFile)) {
                    byte[] buffer = new byte[8192];
                    int read;
                    while ((read = tarIn.read(buffer)) != -1) {
                        out.write(buffer, 0, read);
                    }
                }
            }
            return true;
        } catch (IOException e) {
            return false;
        }
    }

    /**
     * Drops the archive's own top-level folder, then also drops an {@code espeak-ng-data/}
     * component if present -- see {@link #extractArchive}'s doc for why. Returns {@code null}
     * for entries that are only the top-level folder itself (nothing left after stripping).
     */
    private String stripArchiveLayout(String entryName) {
        String path = entryName.replace('\\', '/');
        int firstSlash = path.indexOf('/');
        if (firstSlash < 0) {
            return null; // a bare file at the archive root, not the expected layout
        }
        String withoutTopLevel = path.substring(firstSlash + 1);
        if (withoutTopLevel.isEmpty()) {
            return null;
        }
        if (withoutTopLevel.startsWith("espeak-ng-data/")) {
            withoutTopLevel = withoutTopLevel.substring("espeak-ng-data/".length());
        }
        return withoutTopLevel.isEmpty() ? null : withoutTopLevel;
    }

    /**
     * Voice archives are small (tens of MB, matching the file sizes documented in
     * k2-fsa/sherpa's own model tables) compared to the multi-hundred-MB Whisper/NLLB
     * files, so unlike {@code Downloader.java} this doesn't need a GUI-polling thread wired
     * through the whole fragment lifecycle -- a simple background poll against
     * {@link DownloadManager.Query} until the download leaves the running/pending state is
     * enough.
     */
    private void pollProgress(DownloadManager downloadManager, long downloadId, Listener listener) {
        new Thread(() -> {
            boolean downloading = true;
            while (downloading) {
                DownloadManager.Query query = new DownloadManager.Query().setFilterById(downloadId);
                try (android.database.Cursor cursor = downloadManager.query(query)) {
                    if (cursor != null && cursor.moveToFirst()) {
                        int statusIndex = cursor.getColumnIndex(DownloadManager.COLUMN_STATUS);
                        int bytesIndex = cursor.getColumnIndex(DownloadManager.COLUMN_BYTES_DOWNLOADED_SO_FAR);
                        int totalIndex = cursor.getColumnIndex(DownloadManager.COLUMN_TOTAL_SIZE_BYTES);
                        int status = cursor.getInt(statusIndex);
                        listener.onProgress(cursor.getLong(bytesIndex), cursor.getLong(totalIndex));
                        downloading = status == DownloadManager.STATUS_RUNNING || status == DownloadManager.STATUS_PENDING;
                    } else {
                        downloading = false;
                    }
                } catch (Exception e) {
                    downloading = false;
                }
                if (downloading) {
                    try {
                        Thread.sleep(300);
                    } catch (InterruptedException e) {
                        Thread.currentThread().interrupt();
                        return;
                    }
                }
            }
        }, "voice-model-download-progress").start();
    }
}
