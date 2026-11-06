#!/usr/bin/env bash
# Encode the clips scripts/video_soak.py plays. They carry audio on purpose:
# a video with an audio track runs on the audio clock.
set -euo pipefail
OUT="${1:-$HOME/.cache/portal-soak}"
mkdir -p "$OUT"
cd "$OUT"
F=(ffmpeg -nostdin -y -hide_banner -loglevel error)
A=(-f lavfi -i sine=frequency=330:sample_rate=48000)
src() { echo "testsrc2=s=$1:r=$2"; }
H=(-c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac -b:a 128k -movflags +faststart)
"${F[@]}" -f lavfi -i "$(src 1920x1080 30)" "${A[@]}" -t 120 "${H[@]}" -refs 5 -g 60 -b:v 8M h264_1080p30.mp4 &
"${F[@]}" -f lavfi -i "$(src 1920x1080 60)" "${A[@]}" -t 60 "${H[@]}" -refs 3 -g 120 -b:v 12M h264_1080p60.mp4 &
"${F[@]}" -f lavfi -i "$(src 1920x1080 30)" "${A[@]}" -t 120 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -row-mt 1 -g 60 -b:v 6M -c:a libopus -b:a 96k vp9_1080p30.webm &
"${F[@]}" -f lavfi -i "$(src 1920x1080 30)" "${A[@]}" -t 120 -c:v libsvtav1 -preset 10 -g 60 -b:v 5M -c:a libopus -b:a 96k av1_1080p30.webm &
"${F[@]}" -f lavfi -i "$(src 854x480 30)" "${A[@]}" -t 30 "${H[@]}" -g 30 -b:v 1500k h264_480p.mp4 &
"${F[@]}" -f lavfi -i "$(src 1280x720 30)" "${A[@]}" -t 30 "${H[@]}" -refs 6 -g 30 -b:v 3M h264_720p.mp4 &
"${F[@]}" -f lavfi -i "$(src 720x1280 30)" "${A[@]}" -t 30 "${H[@]}" -g 30 -b:v 3M h264_portrait.mp4 &
wait
# Segments for the adaptive page: same 2 s GOPs in every rendition, so it can switch anywhere.
hls() { # name, encoder args...
  local name=$1; shift
  "${F[@]}" "$@" -f hls -hls_time 2 -hls_playlist_type vod -hls_segment_type fmp4 \
    -hls_fmp4_init_filename "${name}_init.mp4" -hls_segment_filename "${name}_%03d.m4s" "$name.m3u8"
  rm -f "$name.m3u8"
}
V() { echo -f lavfi -i "testsrc2=s=$1:r=30" -t 60 -an -g 60 -keyint_min 60; }
hls mse_h264_1080 $(V 1920x1080) -sc_threshold 0 -c:v libx264 -preset veryfast -pix_fmt yuv420p -refs 5 -b:v 8M &
hls mse_h264_720 $(V 1280x720) -sc_threshold 0 -c:v libx264 -preset veryfast -pix_fmt yuv420p -refs 5 -b:v 4M &
hls mse_h264_480 $(V 854x480) -sc_threshold 0 -c:v libx264 -preset veryfast -pix_fmt yuv420p -b:v 1500k &
hls mse_vp9_1080 $(V 1920x1080) -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -row-mt 1 -b:v 6M &
hls mse_vp9_480 $(V 854x480) -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -row-mt 1 -b:v 1M &
hls mse_av1_1080 $(V 1920x1080) -c:v libsvtav1 -preset 10 -b:v 5M &
hls mse_av1_480 $(V 854x480) -c:v libsvtav1 -preset 10 -b:v 1M &
hls mse_audio -f lavfi -i sine=frequency=330:sample_rate=48000 -t 60 -vn -c:a aac -b:a 128k &
wait
ls -la "$OUT"
