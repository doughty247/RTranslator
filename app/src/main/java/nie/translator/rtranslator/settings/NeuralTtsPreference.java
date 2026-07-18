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

package nie.translator.rtranslator.settings;

import android.content.Context;
import android.content.SharedPreferences;
import android.util.AttributeSet;

import androidx.annotation.NonNull;
import androidx.preference.Preference;
import androidx.preference.PreferenceViewHolder;
import androidx.preference.SwitchPreference;

import nie.translator.rtranslator.Global;
import nie.translator.rtranslator.R;
import nie.translator.rtranslator.tools.tts.NeuralTtsTier;

/**
 * Toggle for the neural TTS feature (docs/neural-tts-and-punctuation-research.md).
 * Same persistence pattern as {@link ShowOriginalTranscriptionMsgPreference} (writes
 * directly to the {@code "default"} SharedPreferences file under key
 * {@code "enableNeuralTts"}), plus RAM gating: disabled and relabeled if this device's
 * total RAM doesn't clear {@link Global#NEURAL_TTS_LOW_TIER_MIN_RAM_MB}, mirroring how
 * {@code NoticeFragment} already warns about low RAM elsewhere in the app rather than
 * introducing a new pattern for it.
 */
public class NeuralTtsPreference extends SwitchPreference {
    private SettingsFragment fragment;
    private Global global;

    public NeuralTtsPreference(Context context, AttributeSet attrs, int defStyleAttr, int defStyleRes) {
        super(context, attrs, defStyleAttr, defStyleRes);
    }

    public NeuralTtsPreference(Context context, AttributeSet attrs, int defStyleAttr) {
        super(context, attrs, defStyleAttr);
    }

    public NeuralTtsPreference(Context context, AttributeSet attrs) {
        super(context, attrs);
    }

    public NeuralTtsPreference(Context context) {
        super(context);
    }

    @Override
    public void onBindViewHolder(PreferenceViewHolder holder) {
        super.onBindViewHolder(holder);

        if (global != null && global.getMaxEligibleNeuralTtsTier() == NeuralTtsTier.NONE) {
            setEnabled(false);
            setChecked(false);
            setSummary(R.string.preference_description_neural_tts_ram_ineligible);
        }

        setOnPreferenceChangeListener(new OnPreferenceChangeListener() {
            @Override
            public boolean onPreferenceChange(Preference preference, Object newValue) {
                if (global != null) {
                    final SharedPreferences sharedPreferences = global.getSharedPreferences("default", Context.MODE_PRIVATE);
                    SharedPreferences.Editor editor = sharedPreferences.edit();
                    editor.putBoolean("enableNeuralTts", (Boolean) newValue);
                    editor.apply();
                }
                return true;
            }
        });
    }

    public void setFragment(@NonNull SettingsFragment fragment) {
        this.fragment = fragment;
        SettingsActivity activity = (SettingsActivity) fragment.requireActivity();
        this.global = (Global) activity.getApplication();
    }
}
