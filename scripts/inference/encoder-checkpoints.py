"""Pinned Whisper/ggml patch: cancellation at encoder layers, including Vulkan."""
from pathlib import Path


def patch(root: Path):
    path=root/'src/whisper.cpp';text=path.read_text()
    begin=text.index('static struct ggml_cgraph * whisper_build_graph_encoder(')
    end=text.index('static struct ggml_cgraph * whisper_build_graph_cross(',begin)
    segment=text[begin:end]
    anchor='        inpL = ggml_add(ctx0, cur, inpFF);'
    assert segment.count(anchor)==1
    segment=segment.replace(anchor,anchor+'\n        ggml_format_name(inpL, "writer_encoder_checkpoint_%d", il);')
    text=text[:begin]+segment+text[end:]
    anchor='''        if (!ggml_graph_compute_helper(sched, gf, n_threads)) {
            return false;
        }
    }

    // cross'''
    assert text.count(anchor)==1
    text=text.replace(anchor,'''        struct writer_cancel { ggml_abort_callback callback; void * data; } cancel{abort_callback, abort_callback_data};
        bool gpu = false;
        auto set_cpu_abort = [&](ggml_abort_callback callback, void * data) {
            for (int i = 0; i < ggml_backend_sched_get_n_backends(sched); ++i) {
                auto backend = ggml_backend_sched_get_backend(sched, i);
                auto device = ggml_backend_get_device(backend);
                if (!device) continue;
                gpu |= ggml_backend_dev_type(device) == GGML_BACKEND_DEVICE_TYPE_GPU || ggml_backend_dev_type(device) == GGML_BACKEND_DEVICE_TYPE_IGPU;
                auto reg = ggml_backend_dev_backend_reg(device);
                auto fn = (ggml_backend_set_abort_callback_t) ggml_backend_reg_get_proc_address(reg, "ggml_backend_set_abort_callback");
                if (fn) fn(backend, callback, data);
            }
        };
        set_cpu_abort(abort_callback, abort_callback_data);
        // GPU dispatch otherwise submits the entire encoder graph before checking
        // cancellation. Observe layer boundaries, without changing any arithmetic.
        if (abort_callback && gpu) {
            ggml_backend_sched_set_eval_callback(sched, [](ggml_tensor * tensor, bool ask, void * opaque) {
                if (ask) return std::strncmp(tensor->name, "writer_encoder_checkpoint_", 26) == 0;
                auto * state = static_cast<writer_cancel *>(opaque);
                return !state->callback(state->data);
            }, &cancel);
        }
        const bool computed = ggml_graph_compute_helper(sched, gf, n_threads);
        ggml_backend_sched_set_eval_callback(sched, nullptr, nullptr);
        set_cpu_abort(nullptr, nullptr);
        if (!computed || (abort_callback && abort_callback(abort_callback_data))) return false;
    }

    // cross''')
    path.write_text(text)
    path=root/'ggml/src/ggml-backend.cpp';text=path.read_text()
    anchor='''                if (need && !sched->callback_eval(t, false, sched->callback_eval_user_data)) {
                    break;
                }'''
    assert text.count(anchor)==1
    text=text.replace(anchor,'''                if (need && !sched->callback_eval(t, false, sched->callback_eval_user_data)) {
                    // Abort the whole scheduled graph, not only the current split.
                    return GGML_STATUS_ABORTED;
                }''')
    path.write_text(text)
