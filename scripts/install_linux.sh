#!/bin/bash
set -e

detect_pkg_manager() {
    if command -v apt &>/dev/null; then echo "apt"
    elif command -v dnf &>/dev/null; then echo "dnf"
    elif command -v pacman &>/dev/null; then echo "pacman"
    elif command -v zypper &>/dev/null; then echo "zypper"
    elif command -v apk &>/dev/null; then echo "apk"
    else echo "unknown"; fi
}

PKG=$(detect_pkg_manager)

case $PKG in
    apt)
        echo "Detected: apt (Debian/Ubuntu)"
        sudo apt update
        sudo apt install -y ffmpeg ghostscript qpdf poppler-utils pandoc python3-pip
        ;;
    dnf)
        echo "Detected: dnf (Fedora/RHEL)"
        sudo dnf install -y ffmpeg ghostscript qpdf poppler-utils pandoc python3-pip
        ;;
    pacman)
        echo "Detected: pacman (Arch Linux)"
        sudo pacman -S --noconfirm ffmpeg ghostscript qpdf poppler pandoc python-pip
        ;;
    zypper)
        echo "Detected: zypper (openSUSE)"
        sudo zypper install -y ffmpeg ghostscript qpdf poppler-tools pandoc python3-pip
        ;;
    apk)
        echo "Detected: apk (Alpine Linux)"
        sudo apk add --no-cache ffmpeg ghostscript qpdf poppler-utils pandoc py3-pip
        ;;
    *)
        echo "Unknown package manager. Please install dependencies manually:"
        echo "  - ffmpeg + ffprobe"
        echo "  - ghostscript (gs)"
        echo "  - qpdf"
        echo "  - poppler-utils (pdftotext, pdfimages, pdfinfo)"
        echo "  - pandoc"
        echo "  - python3 + pip"
        exit 1
        ;;
esac

pip install --user img2pdf 2>/dev/null || pip3 install --user img2pdf 2>/dev/null || true
echo "Dependencies installed successfully."
