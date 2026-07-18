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

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.os.Bundle
import android.os.IBinder
import android.view.LayoutInflater
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import androidx.fragment.app.Fragment
import nie.translator.rtranslator.R
import nie.translator.rtranslator.voice_translation.VoiceTranslationActivity
import uniffi.adaptive_hybrid_core.Direction
import uniffi.adaptive_hybrid_core.DirectionMode

/**
 * Adaptive Hybrid Mode's fragment -- the "one phone, no second device needed" conversation
 * UI this branch's work has been building toward (see
 * docs/adaptive-hybrid-mode-changelog.md). Binds to [AdaptiveHybridService] and exposes
 * per-direction Live/PushToTalk/Off controls.
 * <p>
 * Deliberately a minimal, code-built layout (matching
 * `nie.translator.rtranslator.tools.tts.VoiceDownloadActivity`'s approach) rather than a
 * polished XML layout + custom views matching the rest of the app's UI conventions --
 * getting the mode reachable and functionally wired was the priority for this pass, not
 * matching WalkieTalkieFragment's UI polish. A real settings screen (per-direction mode
 * spinners integrated into the existing `settings/` package's patterns, live transcript
 * display, etc.) is follow-up work.
 * <p>
 * <b>Unverified in this development environment</b> -- see
 * `AdaptiveHybridService`'s class doc.
 */
class AdaptiveHybridFragment : Fragment() {
    private var service: AdaptiveHybridService? = null
    private var bound = false

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName, binder: IBinder) {
            service = (binder as AdaptiveHybridService.LocalBinder).getService()
            bound = true
        }

        override fun onServiceDisconnected(name: ComponentName) {
            service = null
            bound = false
        }
    }

    override fun onCreateView(inflater: LayoutInflater, container: ViewGroup?, savedInstanceState: Bundle?): View {
        val context = requireContext()
        val root = LinearLayout(context)
        root.orientation = LinearLayout.VERTICAL
        val padding = (16 * resources.displayMetrics.density).toInt()
        root.setPadding(padding, padding, padding, padding)

        root.addView(buildDirectionControls(context, Direction.FIRST_TO_SECOND, getString(R.string.adaptive_hybrid_direction_first_to_second)))
        root.addView(buildDirectionControls(context, Direction.SECOND_TO_FIRST, getString(R.string.adaptive_hybrid_direction_second_to_first)))

        val backButton = Button(context)
        backButton.text = getString(R.string.adaptive_hybrid_back_to_translation)
        backButton.setOnClickListener {
            (requireActivity() as? VoiceTranslationActivity)?.setFragment(VoiceTranslationActivity.TRANSLATION_FRAGMENT)
        }
        root.addView(backButton)

        return root
    }

    private fun buildDirectionControls(context: Context, direction: Direction, label: String): LinearLayout {
        val section = LinearLayout(context)
        section.orientation = LinearLayout.VERTICAL
        val padding = (8 * resources.displayMetrics.density).toInt()
        section.setPadding(0, padding, 0, padding)

        val title = TextView(context)
        title.text = label
        title.textSize = 16f
        section.addView(title)

        val modeRow = LinearLayout(context)
        modeRow.orientation = LinearLayout.HORIZONTAL

        val liveButton = Button(context)
        liveButton.text = getString(R.string.adaptive_hybrid_mode_live)
        liveButton.setOnClickListener { service?.setDirectionMode(direction, DirectionMode.LIVE) }

        val pttButton = Button(context)
        pttButton.text = getString(R.string.adaptive_hybrid_mode_push_to_talk)
        pttButton.setOnClickListener { service?.setDirectionMode(direction, DirectionMode.PUSH_TO_TALK) }

        val offButton = Button(context)
        offButton.text = getString(R.string.adaptive_hybrid_mode_off)
        offButton.setOnClickListener { service?.setDirectionMode(direction, DirectionMode.OFF) }

        modeRow.addView(liveButton)
        modeRow.addView(pttButton)
        modeRow.addView(offButton)
        section.addView(modeRow)

        // Held-down-to-talk button -- only meaningful once this direction is set to
        // PushToTalk (setMode above), but it's always shown rather than conditionally
        // enabled/disabled based on live service state, since the mode buttons above are
        // fire-and-forget calls into the service with no confirmation callback wired back
        // to this fragment yet (see class doc -- this is a minimal first pass).
        val pushToTalkHoldButton = Button(context)
        pushToTalkHoldButton.text = getString(R.string.adaptive_hybrid_hold_to_talk)
        pushToTalkHoldButton.setOnTouchListener { _, event ->
            when (event.action) {
                MotionEvent.ACTION_DOWN -> service?.beginPushToTalk(direction)
                MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> service?.endPushToTalk(direction)
            }
            true
        }
        section.addView(pushToTalkHoldButton)

        return section
    }

    override fun onStart() {
        super.onStart()
        val activity = requireActivity() as VoiceTranslationActivity
        activity.startAdaptiveHybridService()
        activity.bindService(Intent(activity, AdaptiveHybridService::class.java), connection, Context.BIND_ABOVE_CLIENT)
    }

    override fun onStop() {
        super.onStop()
        if (bound) {
            requireActivity().unbindService(connection)
            bound = false
        }
    }
}
