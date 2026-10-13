#!/usr/bin/env python3
"""Cross-build Writer's pinned whisper worker on Linux/WSL; no model changes."""
import argparse,hashlib,json,os,subprocess
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parent))
from neutral_paths import prefix_flags,roots,write_receipt  # noqa: E402


def main():
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('--source',type=Path,required=True);p.add_argument('--output',type=Path,required=True)
 p.add_argument('--backend',choices=['cpu','vulkan'],required=True)
 p.add_argument('--cmake',required=True);p.add_argument('--ninja',required=True)
 p.add_argument('--vulkan-sdk',type=Path);p.add_argument('--glslc',type=Path)
 args=p.parse_args();source=args.source.resolve();output=args.output.resolve();output.mkdir(parents=True,exist_ok=True)
 llvm=Path(os.environ['WRITER_LLVM_ROOT']);sdk=Path(os.environ['WRITER_WINDOWS_SDK'])
 if not (source/'writer-source.txt').exists():p.error('Use stage-whisper.py first')
 includes=[sdk/'crt/include']+[sdk/'sdk/include'/part for part in ['ucrt','um','shared','winrt']]
 flags='--target=x86_64-pc-windows-msvc '+' '.join(f'/imsvc "{path}"' for path in includes)
 # No build-machine paths in __FILE__ strings (MW-RELEASE-14).
 flags+=' '+' '.join(prefix_flags(roots(whisper_source=source,whisper_build=output,llvm=llvm,windows_sdk=sdk),clang_cl=True))
 link=' '.join(f'/libpath:"{path}"' for path in [sdk/'crt/lib/x86_64',sdk/'sdk/lib/ucrt/x86_64',sdk/'sdk/lib/um/x86_64'])
 compat=output/'compat.cmake'
 compat.write_text('''function(writer_compat)
 if(TARGET whisper-server)
  target_link_options(whisper-server PRIVATE /MANIFEST:EMBED "/MANIFESTINPUT:@MANIFEST@")
  set_property(TARGET whisper-server APPEND PROPERTY LINK_DEPENDS "@MANIFEST@")
 endif()
 if(TARGET ggml-cpu-alderlake AND CMAKE_CXX_COMPILER_ID MATCHES "Clang")
  target_compile_options(ggml-cpu-alderlake PRIVATE /clang:-mavxvnni)
 endif()
endfunction()
cmake_language(DEFER CALL writer_compat)
'''.replace('@MANIFEST@',str(source/'examples/server/writer.manifest')))
 settings={'CMAKE_POLICY_DEFAULT_CMP0091':'NEW','CMAKE_TRY_COMPILE_CONFIGURATION':'Release','CMAKE_C_FLAGS_DEBUG':'','CMAKE_CXX_FLAGS_DEBUG':'','CMAKE_SYSTEM_NAME':'Windows','CMAKE_SYSTEM_PROCESSOR':'AMD64','CMAKE_BUILD_TYPE':'Release','CMAKE_C_COMPILER':llvm/'bin/clang-cl','CMAKE_CXX_COMPILER':llvm/'bin/clang-cl','CMAKE_AR':llvm/'bin/llvm-lib','CMAKE_LINKER':llvm/'bin/lld-link','CMAKE_RC_COMPILER':llvm/'bin/llvm-rc','CMAKE_MT':llvm/'bin/llvm-mt','CMAKE_MAKE_PROGRAM':args.ninja,'CMAKE_C_FLAGS':flags,'CMAKE_CXX_FLAGS':flags+' /EHsc','CMAKE_EXE_LINKER_FLAGS':link,'CMAKE_SHARED_LINKER_FLAGS':link,'CMAKE_MODULE_LINKER_FLAGS':link,'CMAKE_MSVC_RUNTIME_LIBRARY':'MultiThreaded','CMAKE_PROJECT_TOP_LEVEL_INCLUDES':compat,'BUILD_SHARED_LIBS':'ON','WHISPER_BUILD_TESTS':'OFF','WHISPER_BUILD_EXAMPLES':'ON','GGML_BACKEND_DL':'ON','GGML_NATIVE':'OFF','GGML_CPU_ALL_VARIANTS':'ON','GGML_VULKAN':'ON' if args.backend=='vulkan' else 'OFF'}
 if args.backend=='vulkan':
  if not args.vulkan_sdk or not args.glslc:p.error('Vulkan needs existing SDK and native host glslc')
  vulkan=args.vulkan_sdk.resolve()
  settings.update(Vulkan_INCLUDE_DIR=vulkan/'Include',Vulkan_LIBRARY=vulkan/'Lib/vulkan-1.lib',Vulkan_GLSLC_EXECUTABLE=args.glslc.resolve())
  settings['SPIRV-Headers_DIR']=vulkan/'Lib/cmake/SPIRV-Headers'
 env=os.environ.copy();env['PATH']=str(llvm/'bin')+':'+str(Path(args.ninja).parent)+':'+env['PATH']
 command=[args.cmake,'-S',str(source),'-B',str(output),'-G','Ninja']+[f'-D{k}={v}' for k,v in settings.items()]
 subprocess.run(command,env=env,check=True)
 subprocess.run([args.cmake,'--build',str(output),'--target','whisper-server','--parallel','8'],env=env,check=True)
 write_receipt(output/'writer-build.json',{'source':(source/'writer-source.txt').read_text(),'backend':args.backend,'configure':[str(c) for c in command],
  'compiler':subprocess.check_output([str(llvm/'bin/clang-cl'),'--version'],env=env,text=True).strip(),
  'files':{f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in sorted((output/'bin').iterdir()) if f.suffix in ['.exe','.dll']}})
 print(output/'bin/whisper-server.exe')


if __name__=='__main__':main()
