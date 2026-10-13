#include <jni.h>
#include <fvad.h>
#include <array>
#include <fstream>
#include <memory>
#include <vector>
#include <cstdint>
#include <cmath>

static bool periodic(const std::array<int16_t, 320> & pcm) {
    double mean = 0;
    for (size_t i = 0; i < pcm.size(); i += 2) mean += pcm[i];
    mean /= pcm.size() / 2;
    for (size_t lag = 20; lag <= 100; lag += 4) {
        double cross = 0, left = 0, right = 0;
        for (size_t i = lag * 2; i < pcm.size(); i += 2) {
            const double a = pcm[i] - mean, b = pcm[i - lag * 2] - mean;
            cross += a * b; left += a * a; right += b * b;
        }
        if (left > 1 && right > 1 && cross > .90 * std::sqrt(left * right)) return true;
    }
    return false;
}

// Bounded batches preserve one stateful VAD for the entire recording. Kotlin
// folds runs across batch boundaries and can stop classification immediately.
extern "C" JNIEXPORT jboolean JNICALL Java_com_metrix_writer_SilenceNative_scan(
    JNIEnv * env, jobject, jstring audio, jdouble ceiling, jobject consumer) {
    try {
        const char * path = env->GetStringUTFChars(audio, nullptr);
        if (!path) return false;
        std::ifstream input(path, std::ios::binary);
        env->ReleaseStringUTFChars(audio, path);
        std::unique_ptr<Fvad, decltype(&fvad_free)> vad(fvad_new(), fvad_free);
        if (!input || !vad || fvad_set_sample_rate(vad.get(), 16000) != 0) return false;
        fvad_set_mode(vad.get(), 0); // Least aggressive: possible speech always wins.
        const auto type = env->GetObjectClass(consumer);
        const auto accept = env->GetMethodID(type, "accept", "([BI)Z");
        env->DeleteLocalRef(type);
        if (!accept) return false;
        constexpr size_t batch_size = 4096;
        auto batch = env->NewByteArray(batch_size);
        if (!batch) return false;
        input.seekg(44);
        std::array<unsigned char, 640> bytes{};
        std::array<int16_t, 320> pcm{};
        std::array<jbyte, batch_size> flags{};
        size_t count = 0;
        auto send = [&]() {
            env->SetByteArrayRegion(batch, 0, static_cast<jsize>(count), flags.data());
            if (env->ExceptionCheck()) return false;
            const bool proceed = env->CallBooleanMethod(consumer, accept, batch, static_cast<jint>(count));
            count = 0;
            return !env->ExceptionCheck() && proceed;
        };
        while (input.read(reinterpret_cast<char *>(bytes.data()), bytes.size())) {
            double energy = 0;
            bool digital = true;
            for (size_t i = 0; i < pcm.size(); ++i) {
                pcm[i] = static_cast<int16_t>(bytes[i * 2] | (bytes[i * 2 + 1] << 8));
                energy += static_cast<double>(pcm[i]) * pcm[i];
                digital = digital && pcm[i] == 0;
            }
            const int voice = fvad_process(vad.get(), pcm.data(), pcm.size());
            flags[count++] = digital || (voice == 0 && ceiling > 0 && energy <= pcm.size() * ceiling * ceiling && !periodic(pcm));
            if (count == batch_size && !send()) { env->DeleteLocalRef(batch); return false; }
        }
        const bool success = !input.bad() && (!count || send());
        env->DeleteLocalRef(batch);
        return success;
    } catch (...) { return false; } // Keep original on classifier failure.
}
