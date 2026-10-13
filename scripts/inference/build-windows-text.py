#!/usr/bin/env python3
"""Cross-build Writer's pinned llama.cpp server for Windows on Linux/WSL (MW-SEC-26).

Stages the pinned revision from a llama.cpp clone, replaces its vendored
miniaudio with the pinned header (windows-text-source.json), and builds only
llama-server and the libraries it loads. No model changes; no upstream binaries.
"""
import argparse, hashlib, io, json, os, shutil, subprocess, tarfile
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from neutral_paths import prefix_flags, roots, write_receipt  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True, help='llama.cpp Git clone containing the pinned revision')
    p.add_argument('--miniaudio', type=Path, required=True, help='miniaudio.h matching the pin')
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--backend', choices=['cpu', 'vulkan'], required=True)
    p.add_argument('--cmake', required=True); p.add_argument('--ninja', required=True)
    p.add_argument('--vulkan-sdk', type=Path); p.add_argument('--glslc', type=Path)
    args = p.parse_args()
    pin = json.loads((ROOT/'scripts/inference/windows-text-source.json').read_text())
    header = args.miniaudio.read_bytes()
    if digest(header) != pin['miniaudio']['sha256']:
        p.error('miniaudio.h differs from its pin')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    source = output/'source'
    archive = subprocess.check_output(['git', '-C', str(args.source), 'archive', pin['llama']['revision']])
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(source, filter='data')
    vendored = source/'vendor/miniaudio/miniaudio.h'
    original = digest(vendored.read_bytes())
    vendored.write_bytes(header)
    # A standalone ggml configure writes a pkg-config file from a template that
    # llama.cpp does not vendor; nothing here consumes it.
    (source/'ggml/ggml.pc.in').write_text('')
    llvm, sdk = Path(os.environ['WRITER_LLVM_ROOT']), Path(os.environ['WRITER_WINDOWS_SDK'])
    includes = [sdk/'crt/include'] + [sdk/'sdk/include'/part for part in ['ucrt', 'um', 'shared', 'winrt']]
    flags = '--target=x86_64-pc-windows-msvc ' + ' '.join(f'/imsvc "{path}"' for path in includes)
    # No build-machine paths in __FILE__ strings (MW-RELEASE-14).
    flags += ' ' + ' '.join(prefix_flags(roots(llama_source=source, llama_build=output, llvm=llvm, windows_sdk=sdk), clang_cl=True))
    link = ' '.join(f'/libpath:"{path}"' for path in [sdk/'crt/lib/x86_64', sdk/'sdk/lib/ucrt/x86_64', sdk/'sdk/lib/um/x86_64'])
    # Hybrid CRT: the C++ runtime stays static, but every module uses Windows'
    # own universal C runtime (ucrtbase.dll, part of Windows 10/11), so the
    # FILE* that ggml_fopen() returns from ggml-base.dll is valid in llama code.
    link += ' /NODEFAULTLIB:libucrt.lib ucrt.lib'
    # clang-cl does not derive AVX-VNNI from /arch; same shim as build-windows.py.
    compat = output/'compat.cmake'
    compat.write_text('''function(writer_compat)
 if(TARGET ggml-cpu-alderlake AND CMAKE_CXX_COMPILER_ID MATCHES "Clang")
  target_compile_options(ggml-cpu-alderlake PRIVATE /clang:-mavxvnni)
 endif()
endfunction()
cmake_language(DEFER CALL writer_compat)
''')
    settings = {'CMAKE_PROJECT_TOP_LEVEL_INCLUDES': compat, 'CMAKE_POLICY_DEFAULT_CMP0091': 'NEW', 'CMAKE_TRY_COMPILE_CONFIGURATION': 'Release',
                'CMAKE_SYSTEM_NAME': 'Windows', 'CMAKE_SYSTEM_PROCESSOR': 'AMD64', 'CMAKE_BUILD_TYPE': 'Release',
                'CMAKE_C_COMPILER': llvm/'bin/clang-cl', 'CMAKE_CXX_COMPILER': llvm/'bin/clang-cl',
                'CMAKE_AR': llvm/'bin/llvm-lib', 'CMAKE_LINKER': llvm/'bin/lld-link', 'CMAKE_RC_COMPILER': llvm/'bin/llvm-rc',
                'CMAKE_MT': llvm/'bin/llvm-mt', 'CMAKE_MAKE_PROGRAM': args.ninja,
                'CMAKE_C_FLAGS': flags, 'CMAKE_CXX_FLAGS': flags + ' /EHsc',
                'CMAKE_EXE_LINKER_FLAGS': link, 'CMAKE_SHARED_LINKER_FLAGS': link, 'CMAKE_MODULE_LINKER_FLAGS': link,
                'CMAKE_MSVC_RUNTIME_LIBRARY': 'MultiThreaded', 'BUILD_SHARED_LIBS': 'ON',
                # ggml's own thread pool; no separately attributed OpenMP runtime.
                'GGML_OPENMP': 'OFF', 'GGML_BACKEND_DL': 'ON', 'GGML_NATIVE': 'OFF', 'GGML_CPU_ALL_VARIANTS': 'ON',
                'GGML_VULKAN': 'ON' if args.backend == 'vulkan' else 'OFF', 'GGML_RPC': 'OFF',
                'GGML_BUILD_TESTS': 'OFF', 'GGML_BUILD_EXAMPLES': 'OFF',
                'LLAMA_BUILD_TESTS': 'OFF', 'LLAMA_BUILD_EXAMPLES': 'OFF', 'LLAMA_BUILD_TOOLS': 'ON',
                'LLAMA_BUILD_SERVER': 'ON', 'LLAMA_CURL': 'OFF', 'LLAMA_OPENSSL': 'OFF'}
    if args.backend == 'vulkan':
        if not args.vulkan_sdk or not args.glslc:
            p.error('Vulkan needs an existing SDK and a native host glslc')
        vulkan = args.vulkan_sdk.resolve()
        settings.update(Vulkan_INCLUDE_DIR=vulkan/'Include', Vulkan_LIBRARY=vulkan/'Lib/vulkan-1.lib',
                        Vulkan_GLSLC_EXECUTABLE=args.glslc.resolve())
        settings['SPIRV-Headers_DIR'] = vulkan/'Lib/cmake/SPIRV-Headers'
    env = os.environ.copy()
    env['PATH'] = str(llvm/'bin') + ':' + str(Path(args.ninja).parent) + ':' + env['PATH']
    def configure(src, dest, extra):
        command = [args.cmake, '-S', str(src), '-B', str(dest), '-G', 'Ninja'] + [f'-D{k}={v}' for k, v in {**settings, **extra}.items()]
        subprocess.run(command, env=env, check=True)
        return command
    # C++ objects and exceptions must not cross DLL boundaries with a static C++
    # runtime. ggml has a C API and stays shared for run-time backend selection;
    # llama, common, mtmd and the server link statically into llama-server.exe.
    ggml, prefix, build = output/'build-ggml', output/'ggml', output/'build'
    commands = [configure(source/'ggml', ggml, {'BUILD_SHARED_LIBS': 'ON'})]
    subprocess.run([args.cmake, '--build', str(ggml), '--parallel', '8'], env=env, check=True)
    subprocess.run([args.cmake, '--install', str(ggml), '--prefix', str(prefix)], env=env, check=True)
    commands.append(configure(source, build, {'BUILD_SHARED_LIBS': 'OFF', 'LLAMA_USE_SYSTEM_GGML': 'ON',
                                              'ggml_DIR': prefix/'lib/cmake/ggml'}))
    subprocess.run([args.cmake, '--build', str(build), '--target', 'llama-server', '--parallel', '8'], env=env, check=True)
    package = output/'package'
    package.mkdir()
    shutil.copy2(build/'bin/llama-server.exe', package)
    for library in sorted(set((ggml/'bin').glob('*.dll'))):
        shutil.copy2(library, package)
    command = commands
    files = {f.name: digest(f.read_bytes()) for f in sorted(package.iterdir())}
    write_receipt(output/'writer-build.json', {
        'llama': pin['llama'], 'miniaudio': pin['miniaudio'], 'vendored_miniaudio_sha256': original,
        'backend': args.backend, 'configure': [[str(c) for c in step] for step in command],
        'compiler': subprocess.check_output([str(llvm/'bin/clang-cl'), '--version'], env=env, text=True).strip(),
        'files': files})
    print(package/'llama-server.exe')


if __name__ == '__main__':
    main()
