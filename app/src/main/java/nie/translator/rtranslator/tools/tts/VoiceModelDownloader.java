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
import android.os.Environment;

import java.io.File;

import nie.translator.rtranslator.tools.FileTools;

/**
 * Downloads one {@link NeuralTtsManager.VoiceEntry}'s model files. Deliberately separate
 * from {@code access/Downloader.java} rather than extending it -- that class's
 * {@code DOWNLOAD_URLS}/{@code DOWNLOAD_NAMES}/{@code DOWNLOAD_SIZES} arrays are iterated
 * strictly by index with no per-file skip/opt-out concept (confirmed by reading
 * {@code DownloadReceiver.internalCheckAndStartNextDownload}), which is the right shape
 * for the mandatory first-run Whisper/NLLB download but the wrong shape for an optional,
 * user-triggered, one-language-at-a-time download -- bolting that on risked destabilizing
 * the app's critical first-run flow for a much lower-stakes feature. This class is
 * intentionally simpler: one file at a time, no cross-file progress bookkeeping.
 * <p>
 * Like the existing flow, downloads land in external app storage first (the only place
 * {@link DownloadManager} can target directly) and are then moved into internal storage
 * via {@link FileTools#moveFile}, matching {@code DownloadFragment}'s own external ->
 * internal staging step.
 */
public class VoiceModelDownloader {
    // Placeholder base URL -- point at wherever the voice models referenced by
    // NeuralTtsManager.VOICE_CATALOG actually get hosted (e.g. a GitHub release, same
    // pattern as the existing Whisper/NLLB download URLs in DownloadFragment.java).
    private static final String BASE_URL = "https://github.com/niedev/RTranslator/releases/download/neural-tts-voices/";

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
        downloadOne(entry.languageCode, entry.modelFileName, () ->
                downloadOne(entry.languageCode, entry.tokensFileName, () -> listener.onSuccess(), listener)
        , listener);
    }

    private void downloadOne(String languageCode, String fileName, Runnable onFileSuccess, Listener listener) {
        DownloadManager downloadManager = (DownloadManager) context.getSystemService(Context.DOWNLOAD_SERVICE);
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

                File externalFile = new File(context.getExternalFilesDir(null), fileName);
                File targetDir = new File(new File(context.getFilesDir(), "tts_models"), languageCode);
                if (!targetDir.exists() && !targetDir.mkdirs()) {
                    listener.onFailure();
                    return;
                }
                File targetFile = new File(targetDir, fileName);
                if (!externalFile.exists()) {
                    listener.onFailure();
                    return;
                }
                FileTools.moveFile(externalFile, targetFile, new FileTools.MoveFileCallback() {
                    @Override
                    public void onSuccess() {
                        onFileSuccess.run();
                    }

                    @Override
                    public void onFailure() {
                        listener.onFailure();
                    }
                });
            }
        };
        context.registerReceiver(receiver, new IntentFilter(DownloadManager.ACTION_DOWNLOAD_COMPLETE));
    }

    /**
     * Voice models are small (tens of MB, per docs/neural-tts-and-punctuation-research.md)
     * compared to the multi-hundred-MB Whisper/NLLB files, so unlike {@code Downloader.java}
     * this doesn't need a GUI-polling thread wired through the whole fragment lifecycle --
     * a simple background poll against {@link DownloadManager.Query} until the download
     * leaves the running/pending state is enough.
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
