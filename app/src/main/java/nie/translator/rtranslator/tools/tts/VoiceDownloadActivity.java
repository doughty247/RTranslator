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

import android.app.Activity;
import android.os.Bundle;
import android.view.Gravity;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.File;

/**
 * Optional setup page for downloading higher-quality neural TTS voices (see
 * docs/neural-tts-and-punctuation-research.md), reachable from Settings ->
 * "Download higher quality voices" ({@code SettingsFragment}'s
 * {@code downloadNeuralTtsVoices} preference) rather than being part of the app's mandatory
 * first-run setup flow (`access/AccessActivity`) -- these voices are an enhancement, not
 * something new users need before the app is usable.
 * <p>
 * Deliberately a plain {@link Activity} building its own view tree in code rather than an
 * XML layout + fragment, to keep this self-contained given how simple the screen is (one
 * row per {@link NeuralTtsManager#VOICE_CATALOG} entry, a download button, a progress
 * bar). If the voice catalog grows substantially this should become a RecyclerView-backed
 * screen matching the rest of the app's UI conventions instead.
 * <p>
 * <b>Unverified in this development environment</b> -- see NeuralTts.java's class doc for
 * why (no Android SDK/NDK here to compile against).
 */
public class VoiceDownloadActivity extends Activity {
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);

        ScrollView scrollView = new ScrollView(this);
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        int padding = dp(16);
        root.setPadding(padding, padding, padding, padding);
        scrollView.addView(root);
        setContentView(scrollView);

        VoiceModelDownloader downloader = new VoiceModelDownloader(this);

        for (NeuralTtsManager.VoiceEntry entry : NeuralTtsManager.VOICE_CATALOG) {
            root.addView(buildVoiceRow(entry, downloader));
        }
    }

    private LinearLayout buildVoiceRow(NeuralTtsManager.VoiceEntry entry, VoiceModelDownloader downloader) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.VERTICAL);
        int padding = dp(8);
        row.setPadding(0, padding, 0, padding);

        TextView label = new TextView(this);
        label.setText(entry.languageCode);
        label.setTextSize(16);

        File modelDir = new File(new File(getFilesDir(), "tts_models"), entry.languageCode);
        boolean alreadyDownloaded = new File(modelDir, entry.modelFileName).exists()
                && new File(modelDir, entry.tokensFileName).exists();

        TextView status = new TextView(this);
        ProgressBar progressBar = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        progressBar.setMax(100);
        progressBar.setVisibility(android.view.View.GONE);

        Button downloadButton = new Button(this);
        downloadButton.setText(alreadyDownloaded
                ? getString(nie.translator.rtranslator.R.string.voice_download_status_downloaded)
                : getString(nie.translator.rtranslator.R.string.voice_download_action_download));
        downloadButton.setEnabled(!alreadyDownloaded);
        downloadButton.setOnClickListener(v -> {
            downloadButton.setEnabled(false);
            progressBar.setVisibility(android.view.View.VISIBLE);
            status.setText(nie.translator.rtranslator.R.string.voice_download_status_downloading);
            downloader.download(entry, new VoiceModelDownloader.Listener() {
                @Override
                public void onProgress(long downloadedBytes, long totalBytes) {
                    runOnUiThread(() -> {
                        if (totalBytes > 0) {
                            progressBar.setProgress((int) (100 * downloadedBytes / totalBytes));
                        }
                    });
                }

                @Override
                public void onSuccess() {
                    runOnUiThread(() -> {
                        progressBar.setVisibility(android.view.View.GONE);
                        downloadButton.setText(nie.translator.rtranslator.R.string.voice_download_status_downloaded);
                        status.setText("");
                    });
                }

                @Override
                public void onFailure() {
                    runOnUiThread(() -> {
                        progressBar.setVisibility(android.view.View.GONE);
                        downloadButton.setEnabled(true);
                        status.setText(nie.translator.rtranslator.R.string.voice_download_status_failed);
                    });
                }
            });
        });

        LinearLayout buttonRow = new LinearLayout(this);
        buttonRow.setOrientation(LinearLayout.HORIZONTAL);
        buttonRow.setGravity(Gravity.CENTER_VERTICAL);
        buttonRow.addView(label);
        buttonRow.addView(downloadButton);

        row.addView(buttonRow);
        row.addView(status);
        row.addView(progressBar);
        return row;
    }

    private int dp(int value) {
        float density = getResources().getDisplayMetrics().density;
        return Math.round(value * density);
    }
}
