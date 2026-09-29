#!/usr/bin/env bash
# measure_rss.sh — замер RSS процессов hds на POSIX/macOS (W0, §4 п.1).
# Аналог tools/measure_rss.ps1 для mac-машины.
#
# Примеры:
#   bash tools/measure_rss.sh idle
#   bash tools/measure_rss.sh watch 30 10
set -u

STATE="${1:-idle}"
SAMPLES="${2:-5}"
INTERVAL="${3:-10}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="$SCRIPT_DIR/parity/measurements"
mkdir -p "$OUT_DIR"
OUT_FILE="$OUT_DIR/$STATE-$(date +%Y%m%d-%H%M%S).json"

classify() {
    # $1 = command line; печатает роль или пусто
    local cmd="$1"
    if echo "$cmd" | grep -Eq 'hds\.cli.*watch|hds/watcher'; then echo watch
    elif echo "$cmd" | grep -Eq 'mcp_start|hds\.mcp_(http|server)'; then echo mcp
    elif echo "$cmd" | grep -Eq 'hds\.ui_server|run_ui'; then echo ui
    elif echo "$cmd" | grep -Eq 'run_index|hds\.cli.*index|hds\.indexer'; then echo index
    elif echo "$cmd" | grep -Eq 'hds\.llama_server'; then echo llama-server-manager
    else echo ""
    fi
}

collect_sample() {
    local n="$1"
    local procs_json=""
    local total=0
    local py_total=0
    local llama_total=0
    local count=0
    # python-процессы
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        pid="$(echo "$line" | awk '{print $1}')"
        rss_kb="$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')"
        cmd="$(ps -o command= -p "$pid" 2>/dev/null)"
        role="$(classify "$cmd")"
        [ -z "$role" ] && continue
        mb=$(echo "scale=1; $rss_kb/1024" | bc)
        total=$(echo "$total + $mb" | bc)
        count=$((count + 1))
        cmd_short="$(echo "$cmd" | sed -E 's/.*python3?(\.exe)?[ ]*//' | cut -c1-120)"
        procs_json="$procs_json{\"pid\":$pid,\"role\":\"$role\",\"ws_mb\":$mb,\"cmd\":\"$cmd_short\"},"
        echo "[$n] $role pid=$pid rss=${mb}МБ $cmd_short" >&2
    done < <(pgrep -f 'python.*(hds|mcp_start|run_index|run_ui)' 2>/dev/null)
    # llama-server (если запущен менеджером)
    while IFS= read -r pid; do
        [ -z "$pid" ] && continue
        rss_kb="$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ')"
        [ -z "$rss_kb" ] && continue
        mb=$(echo "scale=1; $rss_kb/1024" | bc)
        llama_total=$(echo "$llama_total + $mb" | bc)
        total=$(echo "$total + $mb" | bc)
        procs_json="$procs_json{\"pid\":$pid,\"role\":\"llama-server\",\"ws_mb\":$mb,\"cmd\":\"llama-server\"},"
        echo "[$n] llama-server pid=$pid rss=${mb}МБ" >&2
    done < <(pgrep -x llama-server 2>/dev/null)
    py_total=$(echo "$total - $llama_total" | bc)
    procs_json="${procs_json%,}"
    echo "{\"sample\":$n,\"timestamp\":\"$(date +%o)\",\"state\":\"$STATE\",\"total_mb\":$total,\"python_mb\":$py_total,\"llama_mb\":$llama_total,\"processes\":[$procs_json]}"
}

echo "Замер состояния '$STATE': $SAMPLES сэмплов с интервалом ${INTERVAL}с (вывод процесса в stderr)"
samples=""
for i in $(seq 1 "$SAMPLES"); do
    samples="$samples$(collect_sample "$i"),"
    [ "$i" -lt "$SAMPLES" ] && sleep "$INTERVAL"
done
samples="${samples%,}"
printf '{\n "state":"%s",\n "collected":"%s",\n "samples_sec":%s,\n "samples":[%s]\n}\n' \
    "$STATE" "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$INTERVAL" "$samples" > "$OUT_FILE"
echo "Сохранено: $OUT_FILE"