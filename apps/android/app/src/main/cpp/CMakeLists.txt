cmake_minimum_required(VERSION 3.22)
project(writer_phone LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_BUILD_TYPE Release CACHE STRING "Optimized qualified CPU runtime in every APK" FORCE)
set(CMAKE_POSITION_INDEPENDENT_CODE ON)
execute_process(COMMAND git -C "${WHISPER_SOURCE}" rev-parse HEAD OUTPUT_VARIABLE revision OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
execute_process(COMMAND git -C "${WHISPER_SOURCE}" status --porcelain OUTPUT_VARIABLE dirty OUTPUT_STRIP_TRAILING_WHITESPACE COMMAND_ERROR_IS_FATAL ANY)
if(NOT revision STREQUAL "927cfce34f31707e17f2bff35c349632fb9e2c3a" OR NOT dirty STREQUAL "")
  message(FATAL_ERROR "Use the clean MW-63 whisper.cpp v1.9.4 revision")
endif()
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(WHISPER_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(WHISPER_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(WHISPER_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
set(GGML_BACKEND_DL OFF CACHE BOOL "" FORCE)
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
foreach(backend CUDA METAL VULKAN OPENCL HEXAGON RPC BLAS CANN SYCL MUSA HIP OPENVINO VIRTGPU ET ZDNN ZENDNN)
  set(GGML_${backend} OFF CACHE BOOL "" FORCE)
endforeach()
# Keep the source checkout immutable; patch a fresh configure-time build copy.
find_package(Python3 REQUIRED COMPONENTS Interpreter)
get_filename_component(WRITER_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../../../../.." ABSOLUTE)
set(WRITER_WHISPER "${CMAKE_CURRENT_BINARY_DIR}/writer-whisper")
file(COPY "${WHISPER_SOURCE}/" DESTINATION "${WRITER_WHISPER}" PATTERN ".git" EXCLUDE)
# COPY preserves timestamps and can skip a previously patched file. Restore the
# original by content before every configure so repeated builds apply once.
configure_file("${WHISPER_SOURCE}/src/whisper.cpp" "${WRITER_WHISPER}/src/whisper.cpp" COPYONLY)
execute_process(COMMAND "${Python3_EXECUTABLE}"
  "${WRITER_ROOT}/scripts/inference/harden-whisper-model.py" --source "${WRITER_WHISPER}"
  COMMAND_ERROR_IS_FATAL ANY)
set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS
  "${WRITER_ROOT}/scripts/inference/harden-whisper-model.py")
add_subdirectory("${WRITER_WHISPER}" whisper)
add_subdirectory(libfvad/src)
add_library(writer-phone SHARED phone.cpp silence.cpp)
target_link_libraries(writer-phone PRIVATE whisper fvad)
target_compile_options(writer-phone PRIVATE -Wall -Wextra -Werror)

# Explicit debug instrumentation only; release never receives this build-type argument.
if(WRITER_PREPARATION_PROBE)
  target_sources(writer-phone PRIVATE ../../androidTest/cpp/preparation_probe.cpp)
endif()
