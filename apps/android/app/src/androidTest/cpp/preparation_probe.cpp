#include "../../main/cpp/file_pcm.h"
#include <jni.h>
#include <malloc.h>
#include <cstring>

// Debug-only: measure preparation, never create a Whisper context or infer.
extern "C" JNIEXPORT jlong JNICALL Java_com_metrix_writer_PreparationProbe_run(
    JNIEnv * env, jobject, jstring audio, jint mode, jobject checkpoint) {
    try {
        const char * chars = env->GetStringUTFChars(audio, nullptr);
        if (!chars) return 0;
        const std::string path(chars); env->ReleaseStringUTFChars(audio, chars);
        const auto type = env->GetObjectClass(checkpoint);
        const auto accept = env->GetMethodID(type, "accept", "(JJ)V");
        env->DeleteLocalRef(type);
        if (!accept) return 0;
        size_t peak = 0;
        int chunks = 0;
        auto cancelled = [&]() {
            const auto info = mallinfo();
            peak = std::max(peak, info.uordblks);
            ++chunks;
            if (mode == 3 && chunks == 8) return true;
            if (mode == 4 && chunks == 8) throw std::bad_alloc();
            return false;
        };
        auto measure = [&](size_t mapped_bytes) {
            const auto info = mallinfo();
            peak = std::max(peak, info.uordblks);
            env->CallVoidMethod(checkpoint, accept, static_cast<jlong>(peak), static_cast<jlong>(mapped_bytes));
        };
        if (mode == 1) { // Exact former production full-vector decoder, preparation only.
            std::ifstream wav(path, std::ios::binary | std::ios::ate);
            const auto samples = static_cast<size_t>((static_cast<int64_t>(wav.tellg()) - 44) / 2);
            std::vector<float> pcm(samples);
            wav.seekg(44);
            std::array<unsigned char, 8192> buffer{};
            size_t offset = 0;
            while (offset < samples) {
                const auto count = std::min(buffer.size() / 2, samples - offset);
                if (!wav.read(reinterpret_cast<char *>(buffer.data()), count * 2)) throw std::runtime_error("Baseline read failed");
                for (size_t i = 0; i < count; ++i) {
                    int sample = buffer[i * 2] | (buffer[i * 2 + 1] << 8);
                    if (sample >= 32768) sample -= 65536;
                    pcm[offset++] = sample / 32768.0f;
                }
            }
            measure(0);
            return static_cast<jlong>(pcm.size());
        }
        FilePcm pcm(path, cancelled);
        measure(pcm.size() * sizeof(float)); // Untouched mapping: actual preparation boundary.
        if (env->ExceptionCheck()) return 0;
        if (mode == 2) {
            std::ifstream wav(path, std::ios::binary); wav.seekg(44);
            std::array<unsigned char, 8192> input{};
            size_t offset = 0;
            while (offset < pcm.size()) {
                const auto count = std::min(input.size() / 2, pcm.size() - offset);
                if (!wav.read(reinterpret_cast<char *>(input.data()), count * 2)) throw std::runtime_error("Comparison read failed");
                for (size_t i = 0; i < count; ++i) {
                    int sample = input[i * 2] | (input[i * 2 + 1] << 8);
                    if (sample >= 32768) sample -= 65536;
                    const float expected = sample / 32768.0f;
                    if (std::memcmp(&expected, pcm.data() + offset + i, sizeof(float)))
                        throw std::runtime_error("Float sample/order mismatch");
                }
                // Comparison itself must not pin a recording-sized RSS.
                madvise(const_cast<float *>(pcm.data() + offset), count * sizeof(float), MADV_DONTNEED);
                offset += count;
            }
        }
        return static_cast<jlong>(pcm.size());
    } catch (const std::bad_alloc &) {
        env->ThrowNew(env->FindClass("java/io/IOException"), "Injected native allocation failure; audio retained"); return 0;
    } catch (const std::exception & error) {
        env->ThrowNew(env->FindClass("java/io/IOException"), error.what()); return 0;
    }
}
