#!/usr/bin/env bash
# Generates the deterministic test media under fixtures/generated/ (git-ignored).
#
#   scripts/fixtures.sh [small|target|all] [--force]
#
# small   speech clips, a 30 s 4K60 screen capture with cursor events, a 30 s 1080p60 camera clip
# target  the 17 min 4K60 workload: screen, camera and mic tracks, events.jsonl and chapters.json
# all     both (default)
#
# Outputs are skipped when every file exists and matches its sha256 in checksums.txt.
# --force regenerates and reports whether each checksum changed.
# See fixtures/README.md for the layout and the determinism rules.
set -euo pipefail

export LC_ALL=C TZ=UTC
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
SPEC=$ROOT/fixtures/spec
OUT=$ROOT/fixtures/generated
WORK=$OUT/.work
SUMS=$OUT/checksums.txt

VOICE=Samantha
FPS=60
SEG_SECONDS=6
SEG_FRAMES=$((FPS * SEG_SECONDS))
SEG_US=$((SEG_SECONDS * 1000000))
SMALL_REPS=5

# Single frame thread and no thread pool: x265 output does not depend on the core count.
X265="log-level=error:frame-threads=1:pools=none:keyint=$FPS:min-keyint=$FPS:bframes=0:scenecut=0:open-gop=0:repeat-headers=1"
# Deterministic muxing: no encoder or tool tags, no creation times, no metadata.
BITEXACT=(-fflags +bitexact -flags +bitexact -map_metadata -1 -map_chapters -1)
FRAG=(-movflags +frag_keyframe+empty_moov+default_base_moof)

MODE=all
FORCE=0
for arg in "$@"; do
  case $arg in
    small | target | all) MODE=$arg ;;
    --force) FORCE=1 ;;
    *) echo "usage: scripts/fixtures.sh [small|target|all] [--force]" >&2; exit 2 ;;
  esac
done

for tool in ffmpeg ffprobe say awk shasum; do
  command -v "$tool" >/dev/null || { echo "missing tool: $tool" >&2; exit 1; }
done

ff() { ffmpeg -hide_banner -loglevel error -nostdin -y "$@"; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
log() { echo "[fixtures] $*" >&2; }

mkdir -p "$OUT"
touch "$SUMS"

# group_current FILE...: true when every file exists and matches its recorded sha256.
group_current() {
  local f rel
  for f in "$@"; do
    rel=${f#"$OUT"/}
    [[ -f $f ]] || return 1
    [[ $(awk -v p="$rel" '$2 == p { print $1 }' "$SUMS") == "$(sha "$f")" ]] || return 1
  done
}

# record FILE...: replace the recorded sha256 of each file and report changes.
record() {
  local f rel new old tmp
  for f in "$@"; do
    rel=${f#"$OUT"/}
    new=$(sha "$f")
    old=$(awk -v p="$rel" '$2 == p { print $1 }' "$SUMS")
    if [[ -n $old ]]; then
      if [[ $old == "$new" ]]; then log "unchanged $rel"; else log "changed   $rel"; fi
    fi
    tmp=$(mktemp)
    awk -v p="$rel" '$2 != p' "$SUMS" >"$tmp"
    echo "$new  $rel" >>"$tmp"
    sort -k2 "$tmp" >"$SUMS"
    rm -f "$tmp"
  done
}

# ---------------------------------------------------------------- speech

# speak NAME TEXTFILE OUT.wav: macOS say, then mono 48 kHz 16 bit PCM.
speak() {
  local aiff=$WORK/$1.aiff
  say -v "$VOICE" -f "$2" -o "$aiff"
  ff -i "$aiff" -ac 1 -ar 48000 -c:a pcm_s16le "${BITEXACT[@]}" "$3"
  rm -f "$aiff"
}

gen_speech() {
  local files=() name out
  for name in clean retake self-correction filler long-silence mid-pause; do
    files+=("$OUT/speech/$name.wav" "$OUT/speech/$name.txt")
  done
  if ((!FORCE)) && group_current "${files[@]}"; then log "speech up to date"; return; fi
  log "speech clips (voice $VOICE)"
  mkdir -p "$OUT/speech"
  for name in clean retake self-correction filler long-silence mid-pause; do
    out=$OUT/speech/$name
    speak "$name" "$SPEC/$name.txt" "$out.wav"
    # Ground truth: the spoken text without say's [[slnc N]] pause markup.
    sed -E 's/ ?\[\[[^]]*\]\] ?/ /g' "$SPEC/$name.txt" >"$out.txt"
  done
  record "${files[@]}"
}

# ---------------------------------------------------------------- screen and camera segments

# The cursor path, click rings and key presses of one 6 s loop come from one scenario.
# This writes the event log, the ffmpeg position expressions and the other filter fragments for it.
plan_scenario() {
  awk -v fps=$FPS -v nframes=$SEG_FRAMES -v outdir="$WORK" '
    function ease(u) { return u * u * (3 - 2 * u) }
    function emit(t_us, json) { printf "%d\t%d\t%s\n", t_us, ++seq, json > (outdir "/scn.events.unsorted") }
    function round(v) { return int(v + 0.5) }
    $1 == "start" { sx = $2; sy = $3; next }
    $1 == "move"  { nm++; ms[nm] = $2; me[nm] = $3; mx[nm] = $4; my[nm] = $5; next }
    $1 == "click" { nc++; ct[nc] = $2; next }
    $1 == "type"  {
      text = $0; sub(/^type +[0-9.]+ +/, "", text)
      for (i = 1; i <= length(text); i++) { nk++; kt[nk] = $2 + (i - 1) * 0.12; kc[nk] = substr(text, i, 1) }
      next
    }
    END {
      lx = -1; ly = -1
      for (n = 0; n < nframes; n++) {
        t = n / fps
        px = sx; py = sy; fx = sx; fy = sy
        for (m = 1; m <= nm; m++) {
          if (t >= me[m]) { px = mx[m]; py = my[m]; fx = px; fy = py }
          else {
            if (t >= ms[m]) { e = ease((t - ms[m]) / (me[m] - ms[m])); px = fx + (mx[m] - fx) * e; py = fy + (my[m] - fy) * e }
            break
          }
        }
        px = round(px); py = round(py)
        if (px != lx || py != ly) {
          emit(round(n * 1000000 / fps), sprintf("{\"t_us\":%d,\"kind\":\"move\",\"x\":%d,\"y\":%d}", round(n * 1000000 / fps), px, py))
          lx = px; ly = py
        }
        pos_x[n] = px; pos_y[n] = py
      }
      for (c = 1; c <= nc; c++) {
        n = round(ct[c] * fps); if (n >= nframes) n = nframes - 1
        d = round(ct[c] * 1000000); u = d + 100000
        emit(d, sprintf("{\"t_us\":%d,\"kind\":\"down\",\"x\":%d,\"y\":%d}", d, pos_x[n], pos_y[n]))
        emit(u, sprintf("{\"t_us\":%d,\"kind\":\"up\",\"x\":%d,\"y\":%d}", u, pos_x[n], pos_y[n]))
        ring = ring (c > 1 ? "+" : "") sprintf("between(t,%.3f,%.3f)", ct[c], ct[c] + 0.25)
      }
      print ring > (outdir "/scn.ring")
      # The same eased path as an ffmpeg expression of t, so frames never depend on timing.
      xe = sx; ye = sy; fx = sx; fy = sy
      for (m = 1; m <= nm; m++) {
        u = sprintf("clip((t-%.6f)/%.6f,0,1)", ms[m], me[m] - ms[m])
        xe = xe sprintf("+(%d)*(%s)*(%s)*(3-2*(%s))", mx[m] - fx, u, u, u)
        ye = ye sprintf("+(%d)*(%s)*(%s)*(3-2*(%s))", my[m] - fy, u, u, u)
        fx = mx[m]; fy = my[m]
      }
      print "round(" xe ")" > (outdir "/scn.x")
      print "round(" ye ")" > (outdir "/scn.y")
      for (k = 1; k <= nk; k++) {
        d = round(kt[k] * 1000000)
        emit(d, sprintf("{\"t_us\":%d,\"kind\":\"key\",\"key\":\"%s\"}", d, kc[k]))
        if (kc[k] != " ") printf ",drawbox=x=%d:y=1672:w=30:h=56:c=white:t=fill:enable=gte(t\\,%.3f)", 360 + (k - 1) * 44, kt[k] > (outdir "/scn.keys")
      }
    }
  ' "$SPEC/screen-scenario.txt"
  touch "$WORK/scn.keys"
  sort -t "$(printf '\t')" -k1,1n -k2,2n "$WORK/scn.events.unsorted" | cut -f3 >"$WORK/seg.events.jsonl"
}

# tile_events REPS OUT: repeat the loop's events REPS times, shifted by 6 s each.
tile_events() {
  awk -v reps="$1" -v seg_us=$SEG_US '
    { lines[NR] = $0 }
    END {
      for (k = 0; k < reps; k++)
        for (i = 1; i <= NR; i++) {
          match(lines[i], /"t_us":[0-9]+/)
          t = substr(lines[i], RSTART + 7, RLENGTH - 7)
          printf "{\"t_us\":%d%s\n", t + k * seg_us, substr(lines[i], RSTART + RLENGTH)
        }
    }
  ' "$WORK/seg.events.jsonl" >"$2"
}

# render_screen_segment ACCENT OUT: 6 s of 4K60 HEVC, an editor-like screen with a video
# window, typed keys, click rings and the cursor from the scenario.
render_screen_segment() {
  local accent=$1 out=$2 bg=$WORK/bg-$1.png
  local f="color=c=0x1b1e28:s=3840x2160,drawbox=x=0:y=0:w=3840:h=80:c=$accent:t=fill"
  f+=",drawbox=x=0:y=80:w=600:h=2080:c=0x232736:t=fill"
  f+=",drawbox=x=300:y=1640:w=1500:h=120:c=0x10121a:t=fill"
  f+=",drawbox=x=300:y=1640:w=8:h=120:c=$accent:t=fill"
  f+=",$(awk 'BEGIN { s = 7; for (i = 0; i < 28; i++) { s = (s * 1103515245 + 12345) % 2147483648; w = 300 + int(s / 65536) % 900; printf ",drawbox=x=%d:y=%d:w=%d:h=18:c=%s:t=fill", 700 + (i % 4) * 60, 140 + i * 48, w, (i % 3 == 0 ? "0x7aa2f7" : (i % 3 == 1 ? "0x9ece6a" : "0xc0caf5")) } }' | cut -c2-)"
  ff -f lavfi -i "$f" -frames:v 1 -update 1 "$bg"

  local keys ring px py
  keys=$(cat "$WORK/scn.keys")
  ring=$(cat "$WORK/scn.ring")
  px=$(cat "$WORK/scn.x")
  py=$(cat "$WORK/scn.y")
  local graph="[0:v]scale=in_range=full:out_range=limited:out_color_matrix=bt709,format=yuv420p[bg];"
  graph+="[bg][1:v]overlay=2000:300${keys}[scene];"
  graph+="color=c=black@0:s=72x72:r=$FPS,format=rgba,drawbox=x=0:y=0:w=72:h=72:c=0xff3b30@1:t=8[ringsrc];"
  graph+="color=c=black:s=26x26:r=$FPS,drawbox=x=3:y=3:w=20:h=20:c=white:t=fill,format=rgba[cur];"
  graph+="[scene][ringsrc]overlay=x='$px-24':y='$py-24':format=auto:enable='$ring'[s2];"
  graph+="[s2][cur]overlay=x='$px':y='$py':format=auto,format=yuv420p[v]"
  ff -loop 1 -framerate $FPS -t $SEG_SECONDS -i "$bg" \
    -f lavfi -i "testsrc2=s=1600x900:r=$FPS:d=$SEG_SECONDS" \
    -filter_complex "$graph" -map '[v]' -frames:v $SEG_FRAMES -r $FPS \
    -c:v libx265 -preset superfast -crf 24 -x265-params "$X265" -tag:v hvc1 \
    -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv \
    "${BITEXACT[@]}" -f mp4 "$out"
}

# render_camera_segment OUT: 6 s of 1080p60 HEVC, a blocky face that blinks and talks.
render_camera_segment() {
  local out=$1
  local face="color=c=0x3a4a6b:s=1920x1080:r=$FPS:d=$SEG_SECONDS"
  face+=",drawbox=x=560:y=800:w=800:h=280:c=0x24304a:t=fill"
  face+=",drawbox=x=880:y=680:w=160:h=140:c=0xc99a7a:t=fill"
  face+=",drawbox=x=700:y=180:w=520:h=560:c=0xe0ac8a:t=fill"
  face+=",drawbox=x=700:y=150:w=520:h=110:c=0x2a1d17:t=fill"
  face+=",drawbox=x=790:y=400:w=100:h=50:c=white:t=fill,drawbox=x=1030:y=400:w=100:h=50:c=white:t=fill"
  face+=",drawbox=x=824:y=408:w=36:h=36:c=0x2a1d17:t=fill,drawbox=x=1064:y=408:w=36:h=36:c=0x2a1d17:t=fill"
  face+=",drawbox=x=790:y=400:w=100:h=50:c=0xe0ac8a:t=fill:enable='between(t,1.9,2.05)+between(t,4.4,4.55)'"
  face+=",drawbox=x=1030:y=400:w=100:h=50:c=0xe0ac8a:t=fill:enable='between(t,1.9,2.05)+between(t,4.4,4.55)'"
  local graph="[0:v]format=yuv420p[face];"
  graph+="[1:v]scale=w=160:h='2*floor(5+7*abs(sin(2*PI*t*2.2)))':eval=frame[mouth];"
  graph+="[face][mouth]overlay=x=880:y='600-h/2':format=yuv420,noise=alls=12:allf=t:all_seed=7,format=yuv420p[v]"
  ff -f lavfi -i "$face" -f lavfi -i "color=c=0x7a2d2d:s=160x20:r=$FPS:d=$SEG_SECONDS" \
    -filter_complex "$graph" -map '[v]' -frames:v $SEG_FRAMES -r $FPS \
    -c:v libx265 -preset superfast -crf 24 -x265-params "$X265" -tag:v hvc1 \
    -colorspace bt709 -color_primaries bt709 -color_trc bt709 -color_range tv \
    "${BITEXACT[@]}" -f mp4 "$out"
}

# tile_video OUT SEGMENT_FOR_INDEX...: stream-copy concatenation into a fragmented MP4.
# Arguments after OUT are segment files, one per repetition, in order.
tile_video() {
  local out=$1 list=$WORK/list-$$-$RANDOM.txt seg
  shift
  : >"$list"
  for seg in "$@"; do printf "file '%s'\n" "$seg" >>"$list"; done
  ff -f concat -safe 0 -i "$list" -c copy -tag:v hvc1 "${FRAG[@]}" "${BITEXACT[@]}" -f mp4 "$out"
  rm -f "$list"
}

repeat_arg() { local i; for ((i = 0; i < $2; i++)); do printf '%s\n' "$1"; done; }

# ---------------------------------------------------------------- groups

gen_small() {
  gen_speech
  local files=("$OUT/screen-30s.mp4" "$OUT/screen-30s.events.jsonl" "$OUT/camera-30s.mp4")
  if ((!FORCE)) && group_current "${files[@]}"; then log "screen and camera clips up to date"; return; fi
  log "30 s screen capture and camera clip"
  plan_scenario
  local accent
  accent=$(awk -F'\t' '$1 == 1 { print $3 }' "$SPEC/chapters.tsv")
  render_screen_segment "$accent" "$WORK/screen-1.mp4"
  render_camera_segment "$WORK/camera.mp4"
  local segs=()
  while IFS= read -r line; do segs+=("$line"); done < <(repeat_arg "$WORK/screen-1.mp4" $SMALL_REPS)
  tile_video "$OUT/screen-30s.mp4" "${segs[@]}"
  segs=()
  while IFS= read -r line; do segs+=("$line"); done < <(repeat_arg "$WORK/camera.mp4" $SMALL_REPS)
  tile_video "$OUT/camera-30s.mp4" "${segs[@]}"
  tile_events $SMALL_REPS "$OUT/screen-30s.events.jsonl"
  record "${files[@]}"
}

gen_target() {
  local dir=$OUT/target
  local files=("$dir/screen.mp4" "$dir/camera.mp4" "$dir/mic.mp4" "$dir/events.jsonl" "$dir/chapters.json")
  if ((!FORCE)) && group_current "${files[@]}"; then log "target up to date"; return; fi
  log "17 min target workload"
  mkdir -p "$dir"
  plan_scenario

  local screen_segs=() cam_segs=() chapters=() start_us=0 reps total_reps=0
  local idx secs accent title speech_len
  render_camera_segment "$WORK/camera.mp4"
  : >"$WORK/mic.list"
  while IFS=$'\t' read -r idx secs accent title; do
    [[ $idx == \#* ]] && continue
    reps=$((secs / SEG_SECONDS))
    log "chapter $idx: $title ($secs s)"
    render_screen_segment "$accent" "$WORK/screen-$idx.mp4"
    local r
    for ((r = 0; r < reps; r++)); do
      screen_segs+=("$WORK/screen-$idx.mp4")
      cam_segs+=("$WORK/camera.mp4")
    done
    # Narration, 0.8 s in, padded with silence to the exact chapter length.
    speak "narration-$idx" "$SPEC/narration-$idx.txt" "$WORK/narration-$idx.raw.wav"
    speech_len=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$WORK/narration-$idx.raw.wav")
    awk -v s="$speech_len" -v c="$secs" 'BEGIN { exit !(s + 0.8 < c) }' ||
      { echo "narration $idx ($speech_len s) does not fit its $secs s chapter" >&2; exit 1; }
    ff -i "$WORK/narration-$idx.raw.wav" -af "adelay=800,apad=whole_dur=$secs" -t "$secs" \
      -c:a pcm_s16le "${BITEXACT[@]}" "$WORK/narration-$idx.wav"
    rm -f "$WORK/narration-$idx.raw.wav"
    echo "file '$WORK/narration-$idx.wav'" >>"$WORK/mic.list"
    chapters+=("$(printf '{"title":"%s","start_us":%d,"end_us":%d}' "$title" "$start_us" "$((start_us + secs * 1000000))")")
    start_us=$((start_us + secs * 1000000))
    total_reps=$((total_reps + reps))
  done <"$SPEC/chapters.tsv"

  tile_video "$dir/screen.mp4" "${screen_segs[@]}"
  tile_video "$dir/camera.mp4" "${cam_segs[@]}"
  ff -f concat -safe 0 -i "$WORK/mic.list" -c:a aac -b:a 96k -ar 48000 -ac 1 \
    -movflags +frag_keyframe+empty_moov+default_base_moof -frag_duration 1000000 \
    "${BITEXACT[@]}" -f mp4 "$dir/mic.mp4"
  tile_events "$total_reps" "$dir/events.jsonl"
  {
    echo "["
    local i
    for ((i = 0; i < ${#chapters[@]}; i++)); do
      ((i + 1 < ${#chapters[@]})) && echo "  ${chapters[i]}," || echo "  ${chapters[i]}"
    done
    echo "]"
  } >"$dir/chapters.json"
  record "${files[@]}"
}

# ---------------------------------------------------------------- main

rm -rf "$WORK"
mkdir -p "$WORK"
trap 'rm -rf "$WORK"' EXIT

start=$SECONDS
case $MODE in
  small) gen_small ;;
  target) gen_target ;;
  all) gen_small; gen_target ;;
esac
log "done in $((SECONDS - start)) s, checksums in ${SUMS#"$ROOT"/}"
