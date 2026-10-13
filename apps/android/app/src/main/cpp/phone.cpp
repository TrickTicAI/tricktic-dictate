#include "whisper.h"
#include "resident_model.h"
#include "file_pcm.h"
#include <jni.h>
#include <atomic>
#include <climits>
#include <cstdint>
#include <fstream>
#include <memory>
#include <mutex>
#include <stdexcept>
#include <string>
#include <thread>
#include <vector>

struct Run { std::atomic<bool> cancelled{false}; };
static std::mutex model_mutex;
static ResidentModel<whisper_context, whisper_free> model_cache;
static bool aborted(void * data) { return static_cast<Run *>(data)->cancelled.load(); }
static void quiet(enum ggml_log_level, const char *, void *) {} // Never log audio-derived text.
static std::string utf(JNIEnv * env, jstring value) {
    const char * chars = env->GetStringUTFChars(value, nullptr);
    if (!chars) throw std::runtime_error("Cannot read native argument");
    std::string result(chars); env->ReleaseStringUTFChars(value, chars); return result;
}
extern "C" JNIEXPORT jlong JNICALL Java_com_metrix_writer_PhoneNative_create(JNIEnv * env, jobject) {
    try { return reinterpret_cast<jlong>(new Run()); }
    catch (const std::bad_alloc &) {
        env->ThrowNew(env->FindClass("java/io/IOException"), "Cannot allocate phone worker; saved audio retained"); return 0;
    }
}
extern "C" JNIEXPORT void JNICALL Java_com_metrix_writer_PhoneNative_cancel(JNIEnv *, jobject, jlong run) { reinterpret_cast<Run *>(run)->cancelled = true; }
extern "C" JNIEXPORT void JNICALL Java_com_metrix_writer_PhoneNative_destroy(JNIEnv *, jobject, jlong run) { delete reinterpret_cast<Run *>(run); }
extern "C" JNIEXPORT void JNICALL Java_com_metrix_writer_PhoneNative_unload(JNIEnv *, jobject) {
    std::lock_guard<std::mutex> lock(model_mutex);
    model_cache.clear();
}
extern "C" JNIEXPORT jbyteArray JNICALL Java_com_metrix_writer_PhoneNative_transcribe(JNIEnv * env, jobject, jlong handle, jstring model, jstring audio, jstring input) {
    auto * run = reinterpret_cast<Run *>(handle);
    try {
        std::lock_guard<std::mutex> lock(model_mutex);
        whisper_log_set(quiet, nullptr);
        auto path = utf(env, audio), weights = utf(env, model), language = utf(env, input);
        if (language != "en" && language != "he" && language != "mixed") throw std::runtime_error("Unsupported phone language");
        FilePcm pcm(path, [run] { return aborted(run); });
        if (aborted(run)) throw std::runtime_error("Phone inference cancelled");
        auto cp = whisper_context_default_params(); cp.use_gpu = false; cp.flash_attn = true;
        auto * ctx = model_cache.get(weights, [&cp](const char * path) {
            return whisper_init_from_file_with_params_no_state(path, cp);
        });
        if (aborted(run)) throw std::runtime_error("Phone inference cancelled");
        // Never retain a previous dictation's decoder state or transcript.
        std::unique_ptr<whisper_state, decltype(&whisper_free_state)> state(whisper_init_state(ctx), whisper_free_state);
        if (!state) throw std::runtime_error("Phone model state could not allocate; choose a smaller model");
        auto p = whisper_full_default_params(WHISPER_SAMPLING_GREEDY);
        p.n_threads = static_cast<int>(std::min(4u, std::max(1u, std::thread::hardware_concurrency()))); p.language = language == "mixed" ? "auto" : language.c_str();
        p.translate = false; p.no_context = true; p.no_timestamps = true;
        p.print_realtime = false; p.print_progress = false; p.print_timestamps = false;
        p.greedy.best_of = 1; p.temperature_inc = 0; p.abort_callback = aborted; p.abort_callback_user_data = run;
        const int result = whisper_full_with_state(ctx, state.get(), p, pcm.data(), static_cast<int>(pcm.size()));
        if (aborted(run)) throw std::runtime_error("Phone inference cancelled");
        if (result != 0) throw std::runtime_error("Phone inference failed; saved audio retained");
        std::string raw;
        for (int i=0; i<whisper_full_n_segments_from_state(state.get()); ++i) raw += whisper_full_get_segment_text_from_state(state.get(), i);
        if (raw.size() > INT_MAX) throw std::runtime_error("Transcript exceeds Android array capacity");
        auto output = env->NewByteArray(static_cast<jsize>(raw.size()));
        if (output) env->SetByteArrayRegion(output, 0, static_cast<jsize>(raw.size()), reinterpret_cast<const jbyte *>(raw.data()));
        return output;
    } catch (const std::bad_alloc &) {
        env->ThrowNew(env->FindClass("java/io/IOException"), "Native model/inference exceeded available memory; saved audio retained"); return nullptr;
    } catch (const std::exception & error) {
        env->ThrowNew(env->FindClass("java/io/IOException"), error.what()); return nullptr;
    }
}
