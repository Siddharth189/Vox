# Portable CPU baseline for Vox release builds that get distributed.
#
# whisper.cpp/ggml defaults to GGML_NATIVE=ON (-march=native / /arch for the
# build machine). A binary built on an AVX-512 CI runner or dev laptop then
# dies with "illegal instruction" on most users' CPUs. whisper-rs-sys only
# forwards WHISPER_*/CMAKE_* variables to CMake, so this file is injected via
#
#   CMAKE_PROJECT_INCLUDE_BEFORE=<repo>/cmake/portable-cpu.cmake
#
# which runs before ggml's option() calls. With GGML_NATIVE pre-set to OFF,
# ggml enables its portable x86-64 baseline instead: AVX, AVX2, FMA, F16C
# (every x86 CPU since ~2013-2015). On Apple Silicon it builds for the
# compiler's default (M1) baseline, which is also required to build at all
# with current Xcode: ggml's -mcpu=native path uses i8mm intrinsics without
# enabling that target feature. Metal does the heavy lifting there anyway.
#
# Local x86/Linux builds for your own machine don't need this: native is
# fastest.
set(GGML_NATIVE OFF CACHE BOOL "Vox portable release build" FORCE)
