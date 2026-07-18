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

/**
 * Which neural TTS voice quality tiers a device's total RAM qualifies for.
 * See {@link nie.translator.rtranslator.Global#getMaxEligibleNeuralTtsTier()}.
 *
 * NONE: neural TTS not offered at all, system TTS is the only option.
 * LOW: sherpa-onnx's low-quality voices only (~20-30MB models).
 * HIGH: everything, including sherpa-onnx's medium (~60-80MB) and high (~100MB+) voices --
 * both are gated behind the same RAM threshold rather than split into three device tiers,
 * since the app can't measure real peak RAM with Whisper+NLLB+TTS all resident to justify a
 * finer split (see docs/neural-tts-and-punctuation-research.md).
 */
public enum NeuralTtsTier {
    NONE,
    LOW,
    HIGH
}
