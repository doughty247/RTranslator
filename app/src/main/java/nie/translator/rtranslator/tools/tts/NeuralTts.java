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

import android.media.AudioAttributes;
import android.media.AudioFormat;
import android.media.AudioTrack;
import android.speech.tts.UtteranceProgressListener;

import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

import com.k2fsa.sherpa.onnx.GeneratedAudio;
import com.k2fsa.sherpa.onnx.OfflinePunctuation;
import com.k2fsa.sherpa.onnx.OfflinePunctuationConfig;
import com.k2fsa.sherpa.onnx.OfflinePunctuationModelConfig;
import com.k2fsa.sherpa.onnx.OfflineTts;
import com.k2fsa.sherpa.onnx.OfflineTtsConfig;

/**
 * Wraps sherpa-onnx's {@link OfflineTts} (see docs/neural-tts-and-punctuation-research.md
 * for why sherpa-onnx rather than raw Piper) as an alternative to {@link
 * nie.translator.rtranslator.tools.TTS}'s Android system TTS wrapper. Deliberately does
 * NOT touch {@code TTS.java} -- this is a separate class so existing modes (WalkieTalkie,
 * Conversation) are completely unaffected when the neural TTS setting is off (the default),
 * rather than risking a shared code path.
 * <p>
 * sherpa-onnx's {@code generate()} call is synchronous/blocking and returns raw PCM
 * samples rather than playing audio itself (unlike {@code TextToSpeech.speak()}, which
 * queues and plays internally) -- this class runs generation on a single-threaded
 * executor (so multiple {@link #speak} calls queue in order, mirroring {@code
 * TextToSpeech.QUEUE_ADD}'s behavior) and plays the result via {@link AudioTrack}.
 * <p>
 * <b>Unverified in this development environment</b> (see
 * docs/solo-conversation-mode-changelog.md): no Android SDK/NDK here to compile against,
 * and the actual sherpa-onnx {@code .aar}/native library isn't vendored in this repo (see
 * app/libs/README.md) -- this class is written directly against sherpa-onnx's documented
 * Kotlin API (confirmed via a shallow clone of the upstream repo during development, not
 * against a compiled dependency), so treat this as needing a real build to confirm.
 */
public class NeuralTts {
    private final ExecutorService executor = Executors.newSingleThreadExecutor();
    private final UtteranceProgressListener listener;
    private OfflineTts tts;
    private OfflinePunctuation punctuation;
    private AudioTrack audioTrack;
    private volatile boolean released = false;

    /**
     * @param modelDir directory (under {@code Context.getFilesDir()}, matching how
     *                  Whisper/NLLB model files are already staged -- see
     *                  Downloader.java) containing the voice model files this tier needs.
     * @param punctuationModelPath path to the CT-Transformer punctuation model file, or
     *                             {@code null} to skip punctuation restoration (e.g. the
     *                             model doesn't cover the current language -- see
     *                             docs/neural-tts-and-punctuation-research.md's caveat
     *                             about language coverage).
     * @param listener reused directly from {@code VoiceTranslationService.ttsListener} --
     *                  {@link UtteranceProgressListener} is a plain abstract class, so its
     *                  callbacks can be invoked manually here without a real {@code
     *                  TextToSpeech} engine involved, keeping the existing mic
     *                  reactivation / echo-safety-signal wiring in {@code
     *                  VoiceTranslationService.speak()} unchanged regardless of which TTS
     *                  backend produced the audio.
     */
    public NeuralTts(OfflineTtsConfig config, String punctuationModelPath, UtteranceProgressListener listener) {
        this.listener = listener;
        // sherpa-onnx's Kotlin data classes rely on default parameter values, which are
        // NOT visible to Java callers unless the Kotlin source is annotated
        // @JvmOverloads (it isn't, as vendored upstream) -- every constructor call in
        // this file supplies all parameters explicitly rather than relying on a no-arg
        // constructor that doesn't actually exist from Java's perspective. Keep this in
        // mind when extending this class: `new OfflinePunctuationModelConfig()` looks
        // like it should compile (every field has a Kotlin-side default) and will not.
        this.tts = new OfflineTts(null, config);
        if (punctuationModelPath != null) {
            OfflinePunctuationModelConfig punctuationModelConfig =
                    new OfflinePunctuationModelConfig(punctuationModelPath, 1, false, "cpu");
            this.punctuation = new OfflinePunctuation(null, new OfflinePunctuationConfig(punctuationModelConfig));
        }
    }

    public boolean isActive() {
        return !released && tts != null;
    }

    /**
     * Mirrors {@code TTS.speak}'s signature loosely -- {@code utteranceId} is required
     * (unlike system TTS, there's no engine-generated default) since {@code
     * VoiceTranslationService} always passes one explicitly today anyway.
     */
    public void speak(String text, String utteranceId) {
        if (!isActive()) {
            if (listener != null) {
                listener.onError(utteranceId);
            }
            return;
        }
        executor.submit(() -> {
            if (released) {
                return;
            }
            if (listener != null) {
                listener.onStart(utteranceId);
            }
            try {
                String textToSpeak = text;
                if (punctuation != null) {
                    // See docs/neural-tts-and-punctuation-research.md -- this is the
                    // concrete answer to the "punctuation from Whisper" request: restore
                    // punctuation on the *translated* text right before synthesis,
                    // independent of whatever Whisper itself predicted.
                    textToSpeak = punctuation.addPunctuation(text);
                }
                GeneratedAudio audio = tts.generate(textToSpeak, 0, 1.0f);
                playBlocking(audio);
                if (listener != null) {
                    listener.onDone(utteranceId);
                }
            } catch (Exception e) {
                android.util.Log.e("NeuralTts", "generation/playback failed for utterance " + utteranceId, e);
                if (listener != null) {
                    listener.onError(utteranceId);
                }
            }
        });
    }

    private void playBlocking(GeneratedAudio audio) {
        int channelConfig = AudioFormat.CHANNEL_OUT_MONO;
        int minBufferSize = AudioTrack.getMinBufferSize(audio.getSampleRate(), channelConfig, AudioFormat.ENCODING_PCM_FLOAT);
        int bufferSize = Math.max(minBufferSize, audio.getSamples().length * 4);

        audioTrack = new AudioTrack.Builder()
                .setAudioAttributes(new AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build())
                .setAudioFormat(new AudioFormat.Builder()
                        .setEncoding(AudioFormat.ENCODING_PCM_FLOAT)
                        .setSampleRate(audio.getSampleRate())
                        .setChannelMask(channelConfig)
                        .build())
                .setBufferSizeInBytes(bufferSize)
                .setTransferMode(AudioTrack.MODE_STATIC)
                .build();

        audioTrack.write(audio.getSamples(), 0, audio.getSamples().length, AudioTrack.WRITE_BLOCKING);
        audioTrack.play();

        // MODE_STATIC + a single write() means playback duration is deterministic from the
        // sample count; block this worker thread until it's done so onDone() fires only
        // after audio actually finishes, matching TextToSpeech's own onDone() semantics
        // that VoiceTranslationService's mic-reactivation logic depends on.
        long durationMs = (long) (1000.0 * audio.getSamples().length / audio.getSampleRate());
        try {
            Thread.sleep(durationMs + 100);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        audioTrack.stop();
        audioTrack.release();
        audioTrack = null;
    }

    public void stop() {
        executor.submit(() -> {
            if (audioTrack != null) {
                try {
                    audioTrack.stop();
                    audioTrack.release();
                } catch (IllegalStateException ignored) {
                }
                audioTrack = null;
            }
        });
    }

    public void release() {
        released = true;
        stop();
        executor.shutdown();
        if (tts != null) {
            tts.release();
            tts = null;
        }
        if (punctuation != null) {
            punctuation.release();
            punctuation = null;
        }
    }
}
