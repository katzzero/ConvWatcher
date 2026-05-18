Write-Host "Installing ConvWatcher dependencies via Chocolatey..."

if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    Write-Host "Chocolatey not found. Please install it first: https://chocolatey.org/install"
    exit 1
}

choco install -y ffmpeg ghostscript qpdf poppler pandoc python

pip install img2pdf

Write-Host "Dependencies installed successfully."
