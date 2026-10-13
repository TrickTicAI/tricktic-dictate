#!/usr/bin/env python3
"""Apply reviewed model-input guards to the pinned desktop/Android Whisper sources.
This touches only a staged copy, never the supplied upstream checkout.
"""
import argparse
from pathlib import Path


def patch(root):
    source = root/'src/whisper.cpp'
    text, vad = source.read_text().split('struct whisper_vad_context {', 1)
    def replace(old, new, count=1):
        nonlocal text
        if text.count(old) != count:
            raise ValueError('Whisper model patch context changed: '+old[:70])
        text = text.replace(old, new)
    replace('template<typename T>\nstatic void read_safe', '''template<typename T>
static void writer_read_safe(whisper_model_loader * loader, T & dest) {
    if (loader->read(loader->context, &dest, sizeof(T)) != sizeof(T)) {
        throw std::runtime_error("truncated model scalar");
    }
    BYTESWAP_VALUE(dest);
}

template<typename T>
static void read_safe''')
    # Upstream's file callback reports the requested length even at EOF.
    replace('''        fin->read((char *)output, read_size);
        return read_size;''', '''        fin->read((char *)output, read_size);
        return static_cast<size_t>(fin->gcount());''')
    replace('''        assert(hparams.n_text_state == hparams.n_audio_state);''', '''        // Writer supports the published Whisper model family (including turbo).
        // Reject invalid dimensions before allocation, division or tensor arithmetic.
        if (hparams.n_vocab <= 0 || hparams.n_vocab > 51866 ||
            hparams.n_audio_ctx <= 0 || hparams.n_audio_ctx > 1500 ||
            hparams.n_text_ctx <= 0 || hparams.n_text_ctx > 448 ||
            hparams.n_audio_state <= 0 || hparams.n_audio_state > 1280 ||
            hparams.n_text_state != hparams.n_audio_state ||
            hparams.n_audio_head <= 0 || hparams.n_audio_head > 20 ||
            hparams.n_text_head <= 0 || hparams.n_text_head > 20 ||
            hparams.n_audio_state % hparams.n_audio_head != 0 ||
            hparams.n_text_state % hparams.n_text_head != 0 ||
            hparams.n_audio_layer <= 0 || hparams.n_audio_layer > 32 ||
            hparams.n_text_layer <= 0 || hparams.n_text_layer > 32 ||
            (hparams.n_mels != 80 && hparams.n_mels != 128)) {
            throw std::runtime_error("invalid model dimensions");
        }''')
    replace('''        filters.data.resize(filters.n_mel * filters.n_fft);
        loader->read(loader->context, filters.data.data(), filters.data.size() * sizeof(float));''', '''        if (filters.n_mel != model.hparams.n_mels || filters.n_fft != 201) {
            throw std::runtime_error("invalid mel filter dimensions");
        }
        filters.data.resize(size_t(filters.n_mel) * size_t(filters.n_fft));
        const size_t filter_bytes = filters.data.size() * sizeof(float);
        if (loader->read(loader->context, filters.data.data(), filter_bytes) != filter_bytes) {
            throw std::runtime_error("truncated mel filters");
        }''')
    replace('''        read_safe(loader, n_vocab);

        //if''', '''        read_safe(loader, n_vocab);
        if (n_vocab < 0 || n_vocab > model.hparams.n_vocab) {
            throw std::runtime_error("invalid vocabulary size");
        }

        //if''')
    replace('''            read_safe(loader, len);

            if (len > 0)''', '''            read_safe(loader, len);
            if (len > 1024 * 1024) {
                throw std::runtime_error("invalid vocabulary token length");
            }

            if (len > 0)''')
    replace('''                loader->read(loader->context, &tmp[0], tmp.size()); // read to buffer''', '''                if (loader->read(loader->context, tmp.data(), tmp.size()) != tmp.size()) {
                    throw std::runtime_error("truncated vocabulary token");
                }''')
    replace('''            read_safe(loader, n_dims);
            read_safe(loader, length);
            read_safe(loader, ttype);

            if (loader->eof(loader->context)) {
                break;
            }''', '''            const size_t header_bytes = loader->read(loader->context, &n_dims, sizeof(n_dims));
            if (header_bytes == 0 && loader->eof(loader->context)) {
                break; // A clean EOF is valid only between complete tensors.
            }
            if (header_bytes != sizeof(n_dims)) {
                throw std::runtime_error("truncated tensor header");
            }
            BYTESWAP_VALUE(n_dims);
            read_safe(loader, length);
            read_safe(loader, ttype);
            if (length <= 0 || length > 1024 || ttype < 0 || ttype >= GGML_TYPE_COUNT) {
                throw std::runtime_error("invalid tensor header");
            }''')
    replace('''            int32_t nelements = 1;''', '''            int64_t nelements = 1;''')
    replace('''                nelements *= ne[i];''', '''                if (ne[i] <= 0 || nelements > INT64_MAX / ne[i]) {
                    throw std::runtime_error("invalid tensor dimensions");
                }
                nelements *= ne[i];''')
    replace('''            loader->read(loader->context, &tmp[0], tmp.size()); // read to buffer''', '''            if (loader->read(loader->context, tmp.data(), tmp.size()) != tmp.size()) {
                throw std::runtime_error("truncated tensor name");
            }''')
    replace('''            const size_t bpe = ggml_type_size(ggml_type(ttype));''', '''            if (ttype != tensor->type) {
                throw std::runtime_error("tensor type differs from model");
            }
            const size_t bpe = ggml_type_size(ggml_type(ttype));''')
    replace('''                loader->read(loader->context, tensor->data, ggml_nbytes(tensor));''', '''                if (loader->read(loader->context, tensor->data, ggml_nbytes(tensor)) != ggml_nbytes(tensor)) {
                    throw std::runtime_error("truncated tensor data");
                }''')
    replace('''                loader->read(loader->context, read_buf.data(), read_buf.size());''', '''                if (loader->read(loader->context, read_buf.data(), read_buf.size()) != read_buf.size()) {
                    throw std::runtime_error("truncated tensor data");
                }''')
    replace('''            WHISPER_LOG_WARN("%s: WARN no tensors loaded from model file - assuming empty model for testing\\n", __func__);''', '''            throw std::runtime_error("model has no tensors");''')
    replace('''        delete ctx;
        return nullptr;
    }

    loader->close(loader->context);''', '''        whisper_free(ctx); // Release partially allocated tensors/backends on malformed models.
        return nullptr;
    }

    loader->close(loader->context);''')
    start = text.index('static bool whisper_model_load(')
    text = text[:start] + text[start:].replace('read_safe(loader,', 'writer_read_safe(loader,')
    source.write_text(text + 'struct whisper_vad_context {' + vad)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    patch(p.parse_args().source)
