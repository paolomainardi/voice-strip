#!/bin/sh
# Downloads licensed speech for the realistic DSP tests into testdata/speech/.
# Source: LibriSpeech (CC BY 4.0, https://www.openslr.org/12/) via the
# hf-internal-testing/librispeech_asr_dummy validation split. 16 kHz
# originals are resampled to 48 kHz mono float WAV with ffmpeg (soxr).
# Requires: curl, uv (for pyarrow), ffmpeg.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out="$root/testdata/speech"
mkdir -p "$out"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/clean.parquet" \
  "https://huggingface.co/datasets/hf-internal-testing/librispeech_asr_dummy/resolve/main/clean/validation-00000-of-00001.parquet"
uvx --quiet --with pyarrow python - "$tmp/clean.parquet" "$tmp" <<'PY'
import sys, pyarrow.parquet as pq
t = pq.read_table(sys.argv[1]).to_pylist()
for row in t[:8]:
    name = row["id"]
    with open(f"{sys.argv[2]}/{name}.flac", "wb") as f:
        f.write(row["audio"]["bytes"])
    print(name, row["speaker_id"], row["text"][:60])
PY
for f in "$tmp"/*.flac; do
  n=$(basename "$f" .flac)
  ffmpeg -loglevel error -y -i "$f" -af aresample=48000:resampler=soxr -ac 1 -c:a pcm_f32le "$out/$n.wav"
done
cat > "$out/LICENSE.txt" <<'TXT'
LibriSpeech ASR corpus, Vassil Panayotov, Guoguo Chen, Daniel Povey, Sanjeev Khudanpur.
License: CC BY 4.0 (https://creativecommons.org/licenses/by/4.0/). Resampled to 48 kHz.
TXT
ls "$out"
