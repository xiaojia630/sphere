#!/usr/bin/env bash
# Headless render without a GPU: SwiftShader's Vulkan ICD (ships with Chromium) and `--shot` (offscreen, no window).
# usage: scripts/shot-swiftshader.sh --shot look/rust-flat.png [--frames N] [--depth] [--term]
set -euo pipefail
cd "$(dirname "$0")/.."
export VK_ICD_FILENAMES="${VK_ICD_FILENAMES:-/opt/pw-browsers/chromium-1194/chrome-linux/vk_swiftshader_icd.json}"
export WGPU_BACKEND="${WGPU_BACKEND:-vulkan}"
exec target/release/sphere-app "$@"
