# Phone-local qualification harness (MW-63)

This is an Android NDK **command-line benchmark**, separate from Writer's APK.
It runs as the ADB shell user on an explicitly selected device. It is not the
app's offline engine or proof of Android background execution. For the measured S25 Ultra results and remaining qualification limits, see receipts and gaps
and candidate rationale.

## Build and acquire exact assets

Linux host: Python 3.11+, CMake 3.22+, make, Git, ADB and the pinned Android NDK.
The installed SDK's licenses must already be accepted. Install the NDK explicitly
if absent (`sdkmanager 'ndk;28.2.13676358'`). No license-acceptance automation is
included. Builds, weights and speech stay outside the checkout.

```bash
export WRITER_BENCH="$HOME/.local/share/metrix-writer/mw63"
mkdir -p "$WRITER_BENCH"
git clone https://github.com/ggml-org/whisper.cpp.git "$WRITER_BENCH/whisper.cpp"
git -C "$WRITER_BENCH/whisper.cpp" switch --detach 927cfce34f31707e17f2bff35c349632fb9e2c3a
python3 apps/android/benchmark/build.py \
  --source "$WRITER_BENCH/whisper.cpp" --output "$WRITER_BENCH/arm64" \
  --ndk "$ANDROID_HOME/ndk/28.2.13676358"
python3 apps/android/benchmark/device.py fetch-model --model base \
  --directory "$WRITER_BENCH/models"
```

Use a **new** output directory per build. `--abi x86_64` builds a tooling fixture
for the Writer emulator. `assets.json` pins runtime revision, NDK/API, model
repository commits, sizes, SHA-256, quantization and license. The fetcher verifies
both size and SHA before atomically publishing a model filename. Build receipts
include binary/source hashes, compiler version and CMake command. The device
runner verifies the binary and every pushed asset again on the device.

Candidate IDs: `base`, `small-q5_1`, `turbo-q5_0`. Fetch only explicitly selected
candidates. Ivrit-turbo was rejected on 2026-09-14 because its measured phone
latency was unacceptable. It is retained only as provenance in `retired_models`;
do not download, select or run it again. The remaining Hebrew/mixed combinations
are experimental, not replacements qualified for daily use. See the research
note for historical licensing and evaluation sources.

## Prepare private references

Create a suite JSON **outside Git**. Each clip is consented/public 16 kHz mono
PCM16 WAV, the same saved-audio format Writer already records. No transcoding or
resampling happens implicitly. Use absolute paths and SHA-256 of the complete
WAV. The native engine receives a byte-identical copy of its PCM payload, decoded
into float samples. References and raw text stay in the private output directory.

```json
{
  "clips": [
    {
      "id": "he-short-01",
      "wav": "/private/consented/he-short-01.wav",
      "sha256": "<64 hexadecimal characters from sha256sum>",
      "mode": "he",
      "consented": true,
      "provenance": "<speaker/source, consent or public license, collection date>",
      "reference": "<private human reference>"
    }
  ]
}
```

Use `he`, `en`, and `mixed` as **separate clips**; the input hints are respectively
`he`, `en`, and `auto`, always `translate=false`. Auto language detection does not
promise mixed-language preservation. Include names, numbers, long pauses and a
long uninterrupted recording. Label accented English in provenance. Include
the maintainer's voice only with consent. An optional `online_raw_text` on each clip
allows reference-based comparison of an already collected online baseline; this
runner never submits audio to obtain one.

## Explicit offline device run

1. Connect the S25 Ultra over **USB ADB** and authorize debugging on the phone.
   Choose its serial from `adb devices -l`. Turn airplane mode on and Wi-Fi and
   Bluetooth off manually; remove tethering/VPN/network interfaces. The script
   checks these states before staging, before inference and every five seconds.
   It never changes permissions/radios, roots the phone, or restarts ADB.
2. Record screen brightness/state, charge percentage, ambient temperature, phone
   case, competing workloads, keyboard/One UI version, and charging method in
   private operator notes. USB often charges the phone; do not call the observed
   battery delta inference energy.
3. Run each candidate on the same clips and settings, in a new output directory:

```bash
python3 apps/android/benchmark/device.py run --serial YOUR_USB_SERIAL \
  --build "$WRITER_BENCH/arm64" --model base \
  --weights "$WRITER_BENCH/models/base.bin" --suite /private/evaluation.json \
  --output "$WRITER_BENCH/s25-base-01" --threads 4 --repeats 3
```

For sustained load add `--minimum-seconds 600`. This repeats the **whole suite**
until at least ten minutes of workload elapse; it never truncates a recording.
Also run an actual long clip: repeating a short clip does not test long-context
omissions. Compare 2 and 4 threads after cooling to the same starting conditions.
There is no general inference timeout or recording-duration cap; actual limits
are available memory, disk and whisper's signed-int sample count. This baseline
holds one entire float clip plus the model in memory. It is not bounded-memory
long-recording app integration.

`--emulator-fixture` permits an online **emulator only**, prominently labels the
receipt as a fixture, and cannot be used on a physical device. It does not alter
other AVDs. The routine Writer AVD lifecycle is documented in `../README.md`.

## Results, cancellation and measurement boundaries

`receipt.json` identifies device/fingerprint/ABI, model and exact native binary.
`runtime.log` records runtime initialization; `results.jsonl` records startup,
model load, each successful raw transcript, PCM preparation/inference times,
process peak RSS and a terminal completion/cancellation/failure. Results flush
and fsync individually. A fresh invocation measures **fresh-process model load**;
its files may already be in the OS page cache after ADB staging. It is not a
reboot-cold storage measurement. Iteration zero includes the first inference;
subsequent iterations reuse the loaded model. No past transcript is supplied as
context. Greedy best-of-1, no temperature fallback, flash attention and fixed
thread count are recorded for comparison, not tuned for phone quality yet.

`telemetry.jsonl` stores periodic battery, thermal service and network snapshots.
Missing/permission-denied counters remain visible, never become zero. `ru_maxrss`
is Linux/Android process-lifetime peak RSS in KiB, including model/PCM buffers;
it is neither incremental model RAM nor PSS. Dumpsys polling adds overhead and
short spikes may fall between samples. Battery temperature is not SoC temperature.
No claim of app-attributed joules/mWh is made. A later unplugged instrumented APK
or calibrated external power setup is needed if USB charging masks energy.

The binary links only the CPU backend (no GPU, NPU, RPC or server adapter), with
`use_gpu=false`; initialization/system info and build receipt substantiate this.
Radio/address checks are observations, not a packet capture, and five-second
sampling cannot prove that radios never changed between samples. There is no
network fallback or provider key path in this runner.

Ctrl-C requests SIGINT only for the exact process in this run's unique directory.
The whisper abort callback exits without publishing partial decoder text as
success; prior results and original WAV remain intact. Loading may finish before
cancellation is observed. If ADB disconnects or the process is killed, recover
`results.jsonl` and `runtime.log` from `receipt.json`'s `remote_directory` with
`adb -s SERIAL pull`. Inspect its `pid` and `/proc/PID/cmdline` before signalling
it. Do not use `pkill`, reset ADB, or delete recordings. New runs use new directories
and do not overwrite earlier attempts. Staged shell-private audio/models remain
until explicitly removed; they are accessible to the authorized ADB shell, not
normal apps. App-private model storage belongs to MW-64.

`quality-review.json` removes raw text and supplies WER/real-time factor, leaving
meaning, omissions, translation, names and human corrections **null until reviewed**.
WER uses Unicode NFKC/casefold, removes combining marks (including Hebrew niqqud),
and treats punctuation as separators; it is lexical edit distance, not a meaning
score. Inspect raw output against each private reference, count edits and record
correction time manually. Summaries can still contain identifying clip IDs, so
review/sanitize before copying any table into Git. Do not interpret exit 0 or a
summary file as physical-device qualification.

```bash
python3 -m unittest discover -s apps/android/benchmark -p 'test_*.py' -v
python3 apps/android/benchmark/device.py summarize --output /private/completed-run
```

Repeat the real-model emulator smoke (about a minute on the validated host):

```bash
python3 apps/android/benchmark/smoke.py --serial emulator-5588 \
  --build "$WRITER_BENCH/x86_64" --weights "$WRITER_BENCH/models/base.bin" \
  --wav "$WRITER_BENCH/whisper.cpp/samples/jfk.wav" \
  --output "$WRITER_BENCH/new-smoke"
```

It checks real inference, signed PCM extrema, empty/odd PCM rejection,
missing-model failure and cooperative cancellation after a completed result;
original audio and prior output hashes must remain unchanged. It verifies the
upstream public sample's hash and refuses a physical device. This English read
speech sample does not qualify accented English, Hebrew or natural mixed speech.

For the S25 CPU instruction comparison, add `--armv82` to `build.py` and use a
new output directory. This enables `armv8.2-a+fp16+dotprod`; the runner checks
`asimdhp` and `asimddp` on every reported CPU before executing that binary.
It remains CPU inference. Compare receipts at matched thermal conditions before
attributing timing changes to instruction selection.

Public Hebrew preparation (downloads the pinned 387 MB FLEURS test archive):

```bash
python3 apps/android/benchmark/fetch-public-he.py --output /private/fleurs-he
```

This selects the first three Hebrew test rows, verifies archive/reference hashes,
and explicitly converts IEEE-float WAV to PCM16 without resampling. The manifest
records original and converted hashes, dataset revision and conversion. Sources:
[Google FLEURS](https://huggingface.co/datasets/google/fleurs) (CC BY 4.0) and
[the pinned files](https://huggingface.co/datasets/google/fleurs/tree/70bb2e84b976b7e960aa89f1c648e09c59f894dd/data/he_il).
These are public read speech, not the maintainer's microphone or mixed dictation.

For saved desktop PCM16 WAVs at other channel counts/rates, reuse Writer's
existing resampler rather than inventing a benchmark-specific conversion:

```bash
rustc --edition=2021 -O apps/android/benchmark/normalize.rs -o /private/normalize
/private/normalize /private/original.wav /private/normalized.wav
cargo test --locked -p writer-dictation-audio
```

Keep original and normalized hashes in the private suite. The preparer preserves
the original, refuses an existing output, and includes the repository's
`writer-dictation-audio` logic directly. It is a host preparation helper, not a
new Android Rust binding. It handles whole files in host memory.

Set `reference_kind` to `public_reference`, `human_reference`, `prompt_confirmed`
or `model_baseline` as appropriate (`provided_reference` is the default for an
unspecified supplied reference). For saved desktop model output, use
`model_baseline`: the summary emits **baseline_disagreement**, leaves **WER null**,
and does not pretend the old recognizer was ground truth. `online_raw_text`
should only describe an actual same-clip online baseline; a desktop-local output
is not an online comparison.

If the speaker changed the prompt and the actual wording is not confirmed, use
`prompt_unconfirmed`. This emits only `prompt_disagreement`, with WER and online
WER null. Save the original prompt and later correction provenance privately;
resummarize after the speaker supplies the actual reference. Do not score a
speaker's intentional wording changes as recognition errors.
