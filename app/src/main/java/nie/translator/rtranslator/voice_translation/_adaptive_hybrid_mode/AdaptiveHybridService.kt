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

package nie.translator.rtranslator.voice_translation._adaptive_hybrid_mode

import android.content.Context
import android.content.Intent
import android.content.SharedPreferences
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.media.audiofx.AcousticEchoCanceler
import android.os.Binder
import android.os.IBinder
import android.util.Log
import java.util.concurrent.atomic.AtomicBoolean
import nie.translator.rtranslator.Global
import nie.translator.rtranslator.tools.CustomLocale
import nie.translator.rtranslator.voice_translation.VoiceTranslationService
import uniffi.adaptive_hybrid_core.DebugListener
import uniffi.adaptive_hybrid_core.Direction
import uniffi.adaptive_hybrid_core.DirectionMode
import uniffi.adaptive_hybrid_core.GateEvent
import uniffi.adaptive_hybrid_core.HybridConfig
import uniffi.adaptive_hybrid_core.HybridException
import uniffi.adaptive_hybrid_core.HybridSession
import uniffi.adaptive_hybrid_core.TranslationListener

/**
 * Adaptive Hybrid Mode's service: owns the `adaptive-hybrid-core` [HybridSession] (Rust,
 * via the UniFFI bindings in `app/src/main/java/uniffi/adaptive_hybrid_core/`) and the raw
 * [AudioRecord] capture feeding it.
 *
 * Extends [VoiceTranslationService] to reuse its TTS plumbing (including the neural TTS
 * routing added in `speak()`, wake lock, and foreground-notification handling) but
 * deliberately does **not** use that base class's [Recorder]-based capture path or its
 * "always stop the mic during TTS" semantics ([shouldDeactivateMicDuringTTS]) -- this
 * mode's whole point is that a Live direction keeps listening through TTS playback,
 * gated by the Rust-side VAD/echo-safety state machine
 * (`docs/adaptive-hybrid-mode-design.md` §2/§4) instead of a blunt on/off switch here.
 *
 * Binding: unlike [nie.translator.rtranslator.voice_translation._walkie_talkie_mode._walkie_talkie.WalkieTalkieService]
 * and [nie.translator.rtranslator.voice_translation._conversation_mode._conversation.ConversationService],
 * which communicate with their fragments through a `Messenger`-based
 * `ServiceCommunicator`/`CustomServiceConnection` IPC layer, this service exposes a plain
 * local [Binder] -- a deliberate simplification, not an oversight: every service in this
 * app already runs in the main process (no `android:process` in `AndroidManifest.xml`),
 * so the cross-process-safe Messenger protocol those modes use buys nothing here, and
 * replicating that protocol's command/`Bundle` structure was a large amount of additional
 * surface for no functional benefit in this same-process case.
 *
 * <b>Unverified in this development environment</b> -- see
 * `adaptive-hybrid-core/README.md`'s "Building the Android native library" section: the
 * Rust cdylib this class calls into via [HybridSession] was never actually cross-compiled
 * or run here (no Android NDK in this sandbox). This class is written directly against
 * the UniFFI-generated Kotlin API, checked method-by-method against
 * `app/src/main/java/uniffi/adaptive_hybrid_core/adaptive_hybrid_core.kt`'s actual
 * generated signatures, not assumed.
 */
class AdaptiveHybridService : VoiceTranslationService() {

    companion object {
        private const val TAG = "AdaptiveHybridService"
        private const val SAMPLE_RATE = 16000
        private const val CHUNK_DURATION_MS = 100
        private const val CHUNK_SAMPLE_COUNT = SAMPLE_RATE * CHUNK_DURATION_MS / 1000 // 1600
    }

    private val binder = LocalBinder()
    private var hybridSession: HybridSession? = null
    private var audioRecord: AudioRecord? = null
    private var echoCanceler: AcousticEchoCanceler? = null
    private var captureThread: Thread? = null
    private val capturing = AtomicBoolean(false)

    // speak() (inherited from VoiceTranslationService) only receives an opaque
    // utteranceId in the onTTSStart/onTTSDone hooks below, not which Direction it was
    // for -- tracked here so notifyPlaybackWindow (§4) is called on the right direction's
    // gate, not just "some" direction.
    private val utteranceDirections = java.util.concurrent.ConcurrentHashMap<String, Direction>()

    inner class LocalBinder : Binder() {
        fun getService(): AdaptiveHybridService = this@AdaptiveHybridService
    }

    override fun onBind(intent: Intent): IBinder {
        // Deliberately not calling super.onBind() -- VoiceTranslationService's base
        // onBind returns its own Messenger-based binder, which this class's simplified
        // local-Binder approach (see class doc) replaces rather than layers on top of.
        return binder
    }

    /**
     * [VoiceTranslationService]'s one abstract method -- exists to lazily construct the
     * base class's [Recorder]-based `mVoiceRecorder` (see e.g.
     * `WalkieTalkieService.initializeVoiceRecorder()`). This mode uses its own
     * [AudioRecord] capture path instead (see [startCapture]), so this deliberately
     * leaves `mVoiceRecorder` null -- every existing null-check on it in the base class
     * already handles that (it's `@Nullable` there specifically for the
     * missing-mic-permission case).
     */
    override fun initializeVoiceRecorder() {
    }

    override fun onCreate() {
        super.onCreate()
        val global = application as Global
        global.getFirstLanguage(false, object : Global.GetLocaleListener {
            override fun onSuccess(first: CustomLocale) {
                global.getSecondLanguage(false, object : Global.GetLocaleListener {
                    override fun onSuccess(second: CustomLocale) {
                        initializeSession(first, second)
                    }

                    override fun onFailure(reasons: IntArray, value: Long) {
                        Log.e(TAG, "failed to read second language, reasons=${reasons.joinToString()}")
                    }
                })
            }

            override fun onFailure(reasons: IntArray, value: Long) {
                Log.e(TAG, "failed to read first language, reasons=${reasons.joinToString()}")
            }
        })
    }

    private fun initializeSession(first: CustomLocale, second: CustomLocale) {
        val prefs = getSharedPreferences("default", Context.MODE_PRIVATE)
        val config = HybridConfig(
            first.language,
            second.language,
            readDirectionMode(prefs, "adaptiveHybridFirstToSecondMode"),
            readDirectionMode(prefs, "adaptiveHybridSecondToFirstMode"),
        )
        val session = HybridSession(config)
        session.setListener(object : TranslationListener {
            override fun onTranslatedText(direction: Direction, text: String) {
                val targetLanguageCode = config.targetLanguage(direction)
                val targetLocale = if (direction == Direction.FIRST_TO_SECOND) second else first
                val utteranceId = "adaptiveHybrid-${System.currentTimeMillis()}"
                utteranceDirections[utteranceId] = direction
                // targetLocale's language should already match targetLanguageCode; kept
                // as a CustomLocale (not just the code) because speak()/TTS need the full
                // Locale, not just an ISO string.
                speak(text, targetLocale, utteranceId)
            }
        })
        session.setDebugListener(object : DebugListener {
            override fun onDebugEvent(direction: Direction, event: GateEvent, elapsedMs: ULong) {
                // §7's field-instrumentation hook -- logged for now rather than
                // surfaced in a UI, since building a log-file/export flow for this was
                // out of scope for getting the mode reachable at all. See
                // docs/adaptive-hybrid-mode-design.md §7.
                Log.d(TAG, "debug event: direction=$direction event=$event elapsedMs=$elapsedMs")
            }
        })
        hybridSession = session
        session.start()
        startCapture()
    }

    private fun readDirectionMode(prefs: SharedPreferences, key: String): DirectionMode {
        return when (prefs.getString(key, "LIVE")) {
            "PUSH_TO_TALK" -> DirectionMode.PUSH_TO_TALK
            "OFF" -> DirectionMode.OFF
            else -> DirectionMode.LIVE
        }
    }

    /** Called by [AdaptiveHybridFragment] when the user changes a direction's mode. */
    fun setDirectionMode(direction: Direction, mode: DirectionMode) {
        hybridSession?.setMode(direction, mode)
        val key = if (direction == Direction.FIRST_TO_SECOND) "adaptiveHybridFirstToSecondMode" else "adaptiveHybridSecondToFirstMode"
        getSharedPreferences("default", Context.MODE_PRIVATE).edit()
            .putString(key, mode.name)
            .apply()
    }

    fun beginPushToTalk(direction: Direction) {
        hybridSession?.beginPushToTalk(direction)
    }

    fun endPushToTalk(direction: Direction) {
        hybridSession?.endPushToTalk(direction)
    }

    // Echo-safety coordination (§4): called from the onTTSStart/onTTSDone hooks added to
    // VoiceTranslationService specifically for this -- WalkieTalkie/Conversation don't
    // override these, so they're unaffected.
    override fun onTTSStart(utteranceId: String) {
        utteranceDirections[utteranceId]?.let { direction ->
            hybridSession?.notifyPlaybackWindow(direction, true)
        }
    }

    override fun onTTSDone(utteranceId: String) {
        utteranceDirections.remove(utteranceId)?.let { direction ->
            hybridSession?.notifyPlaybackWindow(direction, false)
        }
    }

    override fun onTTSError(utteranceId: String) {
        utteranceDirections.remove(utteranceId)?.let { direction ->
            hybridSession?.notifyPlaybackWindow(direction, false)
        }
    }

    // This mode's mic gating is entirely Rust-side VAD (see class doc) -- the base
    // class's blunt "stop the mic during TTS" would defeat a Live direction's whole
    // purpose (listening through the other direction's playback).
    override fun shouldDeactivateMicDuringTTS(): Boolean = false

    private fun startCapture() {
        if (capturing.get()) {
            return
        }
        val minBufferSize = AudioRecord.getMinBufferSize(
            SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_FLOAT,
        )
        if (minBufferSize <= 0) {
            Log.e(TAG, "AudioRecord.getMinBufferSize failed, not starting capture")
            return
        }
        val record = AudioRecord(
            MediaRecorder.AudioSource.MIC,
            SAMPLE_RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_FLOAT,
            maxOf(minBufferSize, CHUNK_SAMPLE_COUNT * 4),
        )
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            Log.e(TAG, "AudioRecord failed to initialize")
            record.release()
            return
        }
        audioRecord = record

        // Echo-safety (docs/echo-safety-analysis.md): RTranslator has never used AEC
        // before this mode, since WalkieTalkie/Conversation always output to the phone
        // speaker or deactivate the mic during their own TTS. Adaptive Hybrid Mode is the
        // first path in this app that needs it. isAvailable() is device/OEM-dependent --
        // per CLAUDE.md this specifically needs confirming on the Pixel 9 Pro XL, not
        // something verifiable in this sandbox.
        if (AcousticEchoCanceler.isAvailable()) {
            // AudioEffect.setEnabled(Boolean) returns Int (a status code), not Unit, so
            // it doesn't pair with getEnabled() into a settable Kotlin property --
            // called explicitly rather than via `it.enabled = true` property syntax,
            // which would not compile.
            echoCanceler = AcousticEchoCanceler.create(record.audioSessionId)?.also {
                it.setEnabled(true)
            }
            Log.i(TAG, "AcousticEchoCanceler enabled=${echoCanceler?.getEnabled()}")
        } else {
            Log.w(TAG, "AcousticEchoCanceler.isAvailable() == false on this device")
        }

        capturing.set(true)
        record.startRecording()

        val thread = Thread({
            val buffer = FloatArray(CHUNK_SAMPLE_COUNT)
            while (capturing.get()) {
                val read = record.read(buffer, 0, buffer.size, AudioRecord.READ_BLOCKING)
                if (read <= 0) {
                    continue
                }
                val chunk = if (read == buffer.size) buffer.toList() else buffer.copyOfRange(0, read).toList()
                try {
                    hybridSession?.pushAudioChunk(chunk, CHUNK_DURATION_MS.toUInt())
                } catch (e: HybridException) {
                    // isRunning() can go false between the capturing.get() check above and
                    // this call (e.g. right after stop()) -- pushAudioChunk then returns a
                    // HybridError.NotRunning across the FFI boundary as an exception rather
                    // than silently no-op'ing. Not a capture-thread-ending condition on its
                    // own, just a dropped chunk during a start/stop race.
                    Log.w(TAG, "pushAudioChunk failed: ${e.message}")
                }
            }
        }, "AdaptiveHybridCapture")
        captureThread = thread
        thread.start()
    }

    private fun stopCapture() {
        capturing.set(false)
        captureThread?.join(1000)
        captureThread = null
        echoCanceler?.release()
        echoCanceler = null
        audioRecord?.let {
            it.stop()
            it.release()
        }
        audioRecord = null
    }

    override fun onDestroy() {
        stopCapture()
        hybridSession?.stop()
        hybridSession?.destroy()
        hybridSession = null
        super.onDestroy()
    }
}
