#!/bin/bash
set -e

if ! command -v brew &>/dev/null; then
    echo "Homebrew not found. Please install it first: https://brew.sh"
    exit 1
fi

brew install ffmpeg ghostscript qpdf poppler pandoc
pip3 install img2pdf

echo "Dependencies installed successfully."
