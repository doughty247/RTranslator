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

import android.content.Context;
import android.content.SharedPreferences;
import android.speech.tts.UtteranceProgressListener;

import java.io.File;

import com.k2fsa.sherpa.onnx.OfflineTtsConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsVitsModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsMatchaModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsKokoroModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsZipVoiceModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsKittenModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsPocketModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTtsSupertonicModelConfig;

import nie.translator.rtranslator.Global;
import nie.translator.rtranslator.tools.CustomLocale;

/**
 * Decides whether neural TTS should be used for a given language, and builds a {@link
 * NeuralTts} instance from whatever voice model files have been downloaded for it.
 * <p>
 * <b>Scope note:</b> this ships with a small starting voice catalog ({@link #VOICE_CATALOG}
 * below), not coverage for all ~200 languages RTranslator/NLLB supports -- sherpa-onnx
 * voices are normally one model per language (unlike NLLB/Whisper's multilingual models),
 * so full coverage means one download entry per language the developer wants a neural
 * voice for. Extend the catalog as voices are added to {@link
 * nie.translator.rtranslator.access.DownloadFragment}'s optional-download list; this class
 * doesn't hardcode anything that would need to change to add more.
 * <p>
 * <b>Catalog coverage, honestly stated:</b> only the {@code en} entry below is confirmed
 * against upstream documentation (k2-fsa/sherpa's docs/source/onnx/tts/pretrained_models/
 * vits.rst, "en_US-lessac-medium" section) -- this sandbox has no network access to browse
 * the full ~641-asset release page at
 * {@code https://github.com/k2-fsa/sherpa-onnx/releases/tag/tts-models} and confirm exact
 * filenames for other languages, and shipping an entry with a guessed filename would just
 * mean a silent 404 for that language's users. To add another language: find its
 * {@code vits-piper-<lang>_<REGION>-<voice>-<quality>.tar.bz2} entry on that release page
 * (Piper voices follow the same HuggingFace {@code rhasspy/piper-voices} path as their
 * archive name, per docs/source/onnx/tts/piper.rst), confirm the archive contains an
 * {@code <lang>_<REGION>-<voice>-<quality>.onnx} + {@code tokens.txt} + {@code
 * espeak-ng-data/} the same way the {@code en} entry does, and add a {@link VoiceEntry}
 * for it.
 */
public class NeuralTtsManager {
    /** Relative to {@code Context.getFilesDir()}, mirrors Whisper/NLLB's staging convention. */
    private static final String TTS_MODELS_SUBDIR = "tts_models";
    private static final String PUNCTUATION_MODEL_FILENAME = "punctuation-ct-transformer.onnx";

    /**
     * One entry per language with a downloadable neural voice. sherpa-onnx only
     * distributes these voices as a single {@code .tar.bz2} archive per voice (see
     * {@link VoiceModelDownloader}, which downloads and extracts {@code archiveFileName}) --
     * {@code modelFileName} and {@code tokensFileName} are the paths to the model and
     * tokens files *inside* that archive's own top-level folder, once extracted into
     * {@code tts_models/<languageCode>/}. The archive's bundled {@code espeak-ng-data/}
     * directory is extracted alongside them into the same {@code tts_models/<languageCode>/}
     * directory (its contents flattened directly into it, not kept in an
     * {@code espeak-ng-data} subfolder) since that's the layout {@link #create} already
     * passes as VITS's {@code dataDir}.
     */
    public static class VoiceEntry {
        public final String languageCode; // ISO 639-1, matches CustomLocale.getLanguage()
        public final String archiveFileName;
        public final String modelFileName;
        public final String tokensFileName;

        public VoiceEntry(String languageCode, String archiveFileName, String modelFileName, String tokensFileName) {
            this.languageCode = languageCode;
            this.archiveFileName = archiveFileName;
            this.modelFileName = modelFileName;
            this.tokensFileName = tokensFileName;
        }
    }

    public static final VoiceEntry[] VOICE_CATALOG = new VoiceEntry[]{
            // Confirmed real entry (see the class doc's "Catalog coverage" note above):
            // single-speaker, medium-quality Piper voice, documented end-to-end including
            // its exact download URL and archive contents in k2-fsa/sherpa's
            // docs/source/onnx/tts/pretrained_models/vits.rst under
            // "en_US-lessac-medium (English, single-speaker)".
            new VoiceEntry("en", "vits-piper-en_US-lessac-medium.tar.bz2",
                    "en_US-lessac-medium.onnx", "tokens.txt"),
    };

    private final Global global;

    public NeuralTtsManager(Global global) {
        this.global = global;
    }

    public boolean isEnabledInSettings() {
        SharedPreferences prefs = global.getSharedPreferences("default", Context.MODE_PRIVATE);
        return prefs.getBoolean("enableNeuralTts", false);
    }

    public boolean isRamEligible() {
        return global.getMaxEligibleNeuralTtsTier() != NeuralTtsTier.NONE;
    }

    private VoiceEntry findVoice(CustomLocale language) {
        String code = language.getLocale().getLanguage();
        for (VoiceEntry entry : VOICE_CATALOG) {
            if (entry.languageCode.equals(code)) {
                return entry;
            }
        }
        return null;
    }

    private File modelDir(VoiceEntry entry) {
        return new File(new File(global.getFilesDir(), TTS_MODELS_SUBDIR), entry.languageCode);
    }

    /**
     * Whether a neural voice is actually usable right now for this language: setting on,
     * RAM eligible, and the model files for this specific language have been downloaded
     * (see NeuralTtsManager -- extending DownloadFragment's flow is what populates this
     * directory; this class only reads it).
     */
    public boolean isAvailableFor(CustomLocale language) {
        if (!isEnabledInSettings() || !isRamEligible()) {
            return false;
        }
        VoiceEntry entry = findVoice(language);
        if (entry == null) {
            return false;
        }
        File dir = modelDir(entry);
        return new File(dir, entry.modelFileName).exists() && new File(dir, entry.tokensFileName).exists();
    }

    /**
     * Builds a ready-to-use {@link NeuralTts} for this language, or {@code null} if
     * {@link #isAvailableFor} would return false -- callers (VoiceTranslationService) are
     * expected to check that first and fall back to system TTS otherwise.
     */
    public NeuralTts create(CustomLocale language, UtteranceProgressListener listener) {
        if (!isAvailableFor(language)) {
            return null;
        }
        VoiceEntry entry = findVoice(language);
        File dir = modelDir(entry);

        // sherpa-onnx's Kotlin data classes need every constructor parameter supplied
        // explicitly from Java (see NeuralTts.java's constructor comment) -- this is the
        // one place in this file where that applies to the *nested* model configs, not
        // just OfflinePunctuationModelConfig.
        OfflineTtsVitsModelConfig vits = new OfflineTtsVitsModelConfig(
                new File(dir, entry.modelFileName).getAbsolutePath(),
                "", // lexicon: not used for this catalog's voices
                new File(dir, entry.tokensFileName).getAbsolutePath(),
                dir.getAbsolutePath(), // dataDir
                "", // dictDir, unused per sherpa-onnx's own comment
                0.667f, // noiseScale
                0.8f,   // noiseScaleW
                1.0f    // lengthScale
        );

        OfflineTtsModelConfig modelConfig = new OfflineTtsModelConfig(
                vits,
                new OfflineTtsMatchaModelConfig("", "", "", "", "", "", 1.0f, 1.0f),
                new OfflineTtsKokoroModelConfig("", "", "", "", "", "", "", 1.0f),
                new OfflineTtsZipVoiceModelConfig("", "", "", "", "", "", 0.1f, 0.5f, 0.1f, 1.0f),
                new OfflineTtsKittenModelConfig("", "", "", "", 1.0f),
                new OfflineTtsPocketModelConfig("", "", "", "", "", "", "", 50),
                new OfflineTtsSupertonicModelConfig("", "", "", "", "", "", ""),
                2,     // numThreads
                false, // debug
                "cpu"  // provider
        );

        OfflineTtsConfig config = new OfflineTtsConfig(modelConfig, "", "", 1, 0.2f);

        File punctuationModel = new File(new File(global.getFilesDir(), TTS_MODELS_SUBDIR), PUNCTUATION_MODEL_FILENAME);
        String punctuationModelPath = punctuationModel.exists() ? punctuationModel.getAbsolutePath() : null;

        return new NeuralTts(config, punctuationModelPath, listener);
    }
}
