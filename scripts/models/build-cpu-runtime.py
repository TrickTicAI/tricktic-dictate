#!/usr/bin/env python3
"""Build pinned whisper.cpp CPU runtimes with runtime CPU instruction dispatch.
Developer/build-host tool only: users receive the resulting binaries and notices.
Cross-building Windows from WSL uses the same LLVM/xwin inputs as cross-cargo.sh.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

PIN = '371b5a7561823ab2bb32142d2751e35e7534727b'  # whisper.cpp v1.9.3

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--source', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--cmake', default='cmake')
    p.add_argument('--ninja', default='ninja')
    p.add_argument('--llvm-root', type=Path)
    p.add_argument('--windows-sdk', type=Path)
    p.add_argument('--jobs', type=int, default=4)
    args = p.parse_args()
    source, output = args.source.resolve(), args.output.resolve()
    revision = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], text=True).strip()
    dirty = subprocess.check_output(['git', '-C', str(source), 'status', '--porcelain'], text=True).splitlines()
    # Upstream CMake stamps the JS binding version even when only native code is built.
    if dirty == [' M bindings/javascript/package.json']:
        before = json.loads(subprocess.check_output(['git', '-C', str(source), 'show', 'HEAD:bindings/javascript/package.json'], text=True))
        after = json.loads((source/'bindings/javascript/package.json').read_text())
        before['version'] = '1.9.3-dev'
        if before == after: dirty = []
    if revision != PIN or dirty:
        p.error('Use a clean whisper.cpp checkout at ' + PIN)
    if output == source or output.is_relative_to(source): p.error('Build output must be outside the source checkout')
    output.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    configure = [args.cmake, '-S', str(source), '-B', str(output/'build'), '-G', 'Ninja', '-DCMAKE_MAKE_PROGRAM='+args.ninja,
                 '-DCMAKE_BUILD_TYPE=Release', '-DBUILD_SHARED_LIBS=ON', '-DGGML_NATIVE=OFF',
                 '-DGGML_BACKEND_DL=ON', '-DGGML_CPU_ALL_VARIANTS=ON', '-DGGML_CUDA=OFF', '-DGGML_VULKAN=OFF',
                 '-DWHISPER_BUILD_IS_DEV=OFF', '-DWHISPER_BUILD_TESTS=OFF', '-DWHISPER_BUILD_EXAMPLES=ON', '-DWHISPER_BUILD_SERVER=ON',
                 '-DWHISPER_CURL=OFF', '-DGGML_OPENMP=OFF']
    if bool(args.llvm_root) != bool(args.windows_sdk): p.error('Supply both LLVM root and Windows SDK for cross-building')
    if args.llvm_root:
        llvm, sdk = args.llvm_root.resolve(), args.windows_sdk.resolve()
        env['PATH'] = str(llvm/'bin') + os.pathsep + env.get('PATH', '')
        env['LD_LIBRARY_PATH'] = str(llvm/'lib') + os.pathsep + str(llvm.parent/'x86_64-linux-gnu') + os.pathsep + env.get('LD_LIBRARY_PATH', '')
        includes = [sdk/'crt/include'] + [sdk/'sdk/include'/d for d in ['ucrt','um','shared','winrt']]
        libraries = [sdk/'crt/lib/x86_64', sdk/'sdk/lib/ucrt/x86_64', sdk/'sdk/lib/um/x86_64']
        cflags = '--target=x86_64-pc-windows-msvc ' + ' '.join('/imsvc "'+str(path)+'"' for path in includes)
        ldflags = ' '.join('/libpath:"'+str(path)+'"' for path in libraries)
        configure += ['-DCMAKE_SYSTEM_NAME=Windows', '-DCMAKE_SYSTEM_PROCESSOR=AMD64', '-DCMAKE_TRY_COMPILE_TARGET_TYPE=STATIC_LIBRARY',
                      '-DCMAKE_C_COMPILER='+str(llvm/'bin/clang-cl'), '-DCMAKE_CXX_COMPILER='+str(llvm/'bin/clang-cl'), '-DCMAKE_LINKER='+str(llvm/'bin/lld-link'),
                      '-DCMAKE_AR='+str(llvm/'bin/llvm-lib'), '-DCMAKE_RC_COMPILER='+str(llvm/'bin/llvm-rc'), '-DCMAKE_MT='+str(llvm/'bin/llvm-mt'),
                      '-DCMAKE_C_FLAGS='+cflags, '-DCMAKE_CXX_FLAGS='+cflags+' /EHsc',
                      '-DCMAKE_C_FLAGS_RELEASE=/O2 /Ob2 /DNDEBUG', '-DCMAKE_CXX_FLAGS_RELEASE=/O2 /Ob2 /DNDEBUG', '-DCMAKE_EXE_LINKER_FLAGS='+ldflags,
                      '-DCMAKE_SHARED_LINKER_FLAGS='+ldflags, '-DCMAKE_MODULE_LINKER_FLAGS='+ldflags,
                      '-DCMAKE_POLICY_DEFAULT_CMP0091=NEW', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded']
    if args.llvm_root or os.name == 'nt':
        configure += ['-DCMAKE_POLICY_DEFAULT_CMP0091=NEW', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded']
        # UTF-8 argv is essential for model paths under non-English Windows accounts.
        manifest = output/'utf8.manifest'
        manifest.write_text('<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0"><application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings><activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage></windowsSettings></application></assembly>')
        compatibility = output/'windows-compat.cmake'
        compatibility.write_text("""function(writer_windows_compat)
  # v1.9.3's clang-cl AVX-VNNI plugin needs its target flag; the baseline must not.
  if(TARGET ggml-cpu-alderlake AND CMAKE_CXX_COMPILER_ID MATCHES "Clang" AND MSVC)
    target_compile_options(ggml-cpu-alderlake PRIVATE /clang:-mavxvnni)
  endif()
  foreach(binary whisper-server whisper-cli)
    if(TARGET ${binary})
      target_link_options(${binary} PRIVATE /MANIFEST:EMBED "/MANIFESTINPUT:"""+manifest.as_posix()+"""")
    endif()
  endforeach()
endfunction()
cmake_language(DEFER CALL writer_windows_compat)
""")
        configure.append('-DCMAKE_PROJECT_TOP_LEVEL_INCLUDES='+str(compatibility))
    subprocess.run(configure, env=env, check=True)
    # Build explicit runtime-loaded plugins as well as the executables. These cover
    # x64 baseline, SSE4.2 and AVX2/FMA/F16C CPUs without requiring AVX-VNNI/AVX512.
    subprocess.run([args.cmake, '--build', str(output/'build'), '--parallel', str(args.jobs), '--target',
                    'whisper-server', 'whisper-cli', 'ggml-cpu-x64', 'ggml-cpu-sse42', 'ggml-cpu-haswell'], env=env, check=True)
    package = output/'package'
    package.mkdir(exist_ok=True)
    allowed = {'whisper-server.exe', 'whisper-cli.exe', 'whisper-server', 'whisper-cli',
               'ggml.dll', 'ggml-base.dll', 'whisper.dll', 'ggml-cpu-x64.dll', 'ggml-cpu-sse42.dll', 'ggml-cpu-haswell.dll'}
    candidates = [file for file in (output/'build/bin').iterdir() if file.name in allowed or file.name.startswith(('libggml-cpu-x64.so', 'libggml-cpu-sse42.so', 'libggml-cpu-haswell.so'))]
    candidates += list((output/'build/src').glob('libwhisper.so*')) + list((output/'build/ggml/src').glob('libggml*.so*'))
    expected = {file.name for file in candidates} | {'LICENSE-whisper.txt'}
    if any(file.name not in expected for file in package.iterdir()):
        p.error('Package directory contains unrelated files; use a fresh output directory')
    for file in candidates: shutil.copy2(file, package/file.name)
    shutil.copy2(source/'LICENSE', package/'LICENSE-whisper.txt')
    manifest = {'source': 'https://github.com/ggml-org/whisper.cpp', 'revision': PIN, 'backend': 'cpu',
                'instruction_dispatch': 'ggml x64 baseline, SSE4.2 and AVX2/FMA/F16C variants',
                'platform': 'windows-x64' if args.llvm_root or os.name == 'nt' else 'linux-x64',
                'files': [{'path': f.name, 'bytes': f.stat().st_size, 'sha256': hashlib.sha256(f.read_bytes()).hexdigest()} for f in sorted(package.iterdir()) if f.is_file()]}
    (output/'cpu-runtime.json').write_text(json.dumps(manifest, indent=2)+'\n')
    print('CPU runtime package:', package)

if __name__ == '__main__': main()
