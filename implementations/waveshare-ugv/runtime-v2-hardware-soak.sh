#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: runtime-v2-hardware-soak.sh --backend cpu|cuda --pilot-token-file FILE
       --expected-binary-sha256 SHA256 --output NEW_FILE --operator-confirmed
       [options]

Run a supervised low-speed Runtime v2 hardware soak against the active local
Leash service. This script issues non-zero drive commands, verified stops, and
one latching E-stop. It must be run only with a secured chassis or lifted
wheels, a reachable physical E-stop, and a second observer present.

Options:
  --duration-secs N       soak duration before final E-stop (default: 120; min: 60)
  --pulse-ms N            duration of each low-speed pulse (default: 250; max: 500)
  --settle-ms N           zero-motion interval after each pulse (default: 500)
  --drive N               normalized wheel command magnitude (default: 0.08; max: 0.10)
  --service NAME          systemd user service (default: leash.service)
  --base-url URL          local Leash URL (default: http://127.0.0.1:8000)
  --operator-confirmed    assert all on-site physical safety preconditions
  -h, --help              show this help

The token is read from a private file and is never written to evidence.
EOF
}

die() {
  echo "error: $*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || die "$1 is required"
}

backend=""
pilot_token_file=""
expected_binary_sha256=""
output=""
duration_secs=120
pulse_ms=250
settle_ms=500
drive=0.08
service="leash.service"
base_url="http://127.0.0.1:8000"
operator_confirmed=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    --backend) backend="${2:?missing backend}"; shift 2 ;;
    --pilot-token-file) pilot_token_file="${2:?missing token file}"; shift 2 ;;
    --expected-binary-sha256) expected_binary_sha256="${2:?missing SHA-256}"; shift 2 ;;
    --output) output="${2:?missing output}"; shift 2 ;;
    --duration-secs) duration_secs="${2:?missing duration}"; shift 2 ;;
    --pulse-ms) pulse_ms="${2:?missing pulse duration}"; shift 2 ;;
    --settle-ms) settle_ms="${2:?missing settle duration}"; shift 2 ;;
    --drive) drive="${2:?missing drive magnitude}"; shift 2 ;;
    --service) service="${2:?missing service}"; shift 2 ;;
    --base-url) base_url="${2:?missing URL}"; shift 2 ;;
    --operator-confirmed) operator_confirmed=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

for command in awk curl jq readlink sha256sum sort systemctl tegrastats; do
  need "$command"
done
[[ "$backend" =~ ^(cpu|cuda)$ ]] || die "--backend must be cpu or cuda"
[[ -r "$pilot_token_file" ]] || die "a readable --pilot-token-file is required"
[[ "$expected_binary_sha256" =~ ^[0-9a-f]{64}$ ]] || die "expected binary SHA-256 must be lowercase hex"
[[ -n "$output" && ! -e "$output" ]] || die "--output must name a new file"
[[ "$duration_secs" =~ ^[0-9]+$ && "$duration_secs" -ge 60 ]] || die "duration must be at least 60 seconds"
[[ "$pulse_ms" =~ ^[0-9]+$ && "$pulse_ms" -gt 0 && "$pulse_ms" -le 500 ]] || die "pulse must be 1..500 ms"
[[ "$settle_ms" =~ ^[0-9]+$ && "$settle_ms" -ge 250 ]] || die "settle must be at least 250 ms"
awk -v value="$drive" 'BEGIN {exit !(value > 0 && value <= 0.10)}' || die "drive must be >0 and <=0.10"
[[ "$operator_confirmed" == true ]] || die "--operator-confirmed is required after the on-site safety check"

pilot_token="$(tr -d '\r\n' <"$pilot_token_file")"
[[ -n "$pilot_token" ]] || die "pilot token file is empty"
mkdir -p "$(dirname "$output")"
chmod 700 "$(dirname "$output")"
umask 077
latency_file="$(mktemp "${TMPDIR:-/tmp}/leash-rv2-stop-latency.XXXXXX")"
sample_file="$(mktemp "${TMPDIR:-/tmp}/leash-rv2-hardware-samples.XXXXXX")"
tegrastats_file="${output%.json}.tegrastats.log"
[[ ! -e "$tegrastats_file" ]] || die "tegrastats output already exists: $tegrastats_file"
tegrastats_pid=""

stop_robot() {
  curl -fsS --max-time 2 -X POST "$base_url/stop" >/dev/null || true
}

finish() {
  local exit_code=$?
  stop_robot
  if [[ -n "$tegrastats_pid" ]]; then
    kill "$tegrastats_pid" 2>/dev/null || true
    wait "$tegrastats_pid" 2>/dev/null || true
  fi
  rm -f "$latency_file" "$sample_file"
  chmod 600 "$output" "$tegrastats_file" 2>/dev/null || true
  return "$exit_code"
}
trap finish EXIT
trap 'exit 130' INT TERM

[[ "$(systemctl --user is-active "$service")" == "active" ]] || die "$service is not active"
service_pid="$(systemctl --user show "$service" -p MainPID --value)"
[[ "$service_pid" =~ ^[1-9][0-9]*$ ]] || die "$service has no running PID"
binary="$(readlink -f "/proc/$service_pid/exe")"
binary_sha256="$(sha256sum "$binary" | awk '{print $1}')"
[[ "$binary_sha256" == "$expected_binary_sha256" ]] || die "active binary does not match the expected candidate"

health="$(curl -fsS --max-time 2 "$base_url/health")"
cognition="$(curl -fsS --max-time 2 "$base_url/cognition/status")"
runtime_v2="$(curl -fsS --max-time 2 "$base_url/runtime-v2/status")"
jq -e --arg backend "$backend" \
  '.ok == true and .accelerator.requested == $backend and .accelerator.active == $backend and .accelerator.required == true' \
  <<<"$health" >/dev/null || die "health does not report the required active backend"
jq -e --arg backend "$backend" \
  '.ok == true and .backend_status.selected == $backend and .backend_status.active == $backend and .backend_status.degraded == false and .capabilities.motor_authority == false' \
  <<<"$cognition" >/dev/null || die "cognition does not report the required backend without motor authority"
jq -e \
  '.available == true and .cpu_final_authority == true and .control_authority == "cpu-safety-supervisor" and .motor_authority == "waveshare-controller-owner" and .controller.connected == true and .controller.estopped == false and .supervisor.faulted == false and .supervisor.evidence.healthy == true' \
  <<<"$runtime_v2" >/dev/null || die "Runtime v2 physical authority is not healthy"

tegrastats --interval 100 >"$tegrastats_file" 2>&1 &
tegrastats_pid=$!
sleep 0.2
kill -0 "$tegrastats_pid" 2>/dev/null || die "tegrastats did not start"

verified_stop() {
  curl -fsS --max-time 2 -X POST -H 'Content-Type: application/json' \
    --data '{"reason":"operator-request"}' "$base_url/stop/verified"
}

authorize() {
  local payload
  payload="$(jq -nc --arg token "$pilot_token" '{token:$token,ttl_secs:30,speed_mode:"low"}')"
  curl -fsS --max-time 2 -X POST -H 'Content-Type: application/json' --data "$payload" \
    "$base_url/pilot/authorize" >/dev/null
}

drive_pulse() {
  local payload
  payload="$(jq -nc --arg token "$pilot_token" --argjson drive "$drive" \
    '{token:$token,left:$drive,right:$drive,speed_mode:"low",approval:true}')"
  curl -fsS --max-time 2 -X POST -H 'Content-Type: application/json' --data "$payload" \
    "$base_url/drive"
}

initial_zero="$(verified_stop)"
jq -e '.acknowledged == true' <<<"$initial_zero" >/dev/null || die "initial verified zero was not acknowledged"
baseline_status="$(curl -fsS --max-time 2 "$base_url/runtime-v2/status")"
previous_stop_sequence="$(jq -r '.controller.last_stop_receipt.applied_sequence' <<<"$baseline_status")"
baseline_write_failures="$(jq -r '.controller.metrics.write_failures' <<<"$baseline_status")"
baseline_ack_failures="$(jq -r '.supervisor.metrics.acknowledgement_failures' <<<"$baseline_status")"
baseline_faults="$(jq -r '.supervisor.metrics.faults' <<<"$baseline_status")"
baseline_evidence_failures="$(jq -r '.supervisor.metrics.evidence_failures' <<<"$baseline_status")"

started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
started_epoch="$(date +%s)"
deadline=$((started_epoch + duration_secs))
pulses=0
pulse_sleep="$(awk -v ms="$pulse_ms" 'BEGIN {printf "%.3f", ms / 1000}')"
settle_sleep="$(awk -v ms="$settle_ms" 'BEGIN {printf "%.3f", ms / 1000}')"

while (( $(date +%s) < deadline )); do
  [[ "$(systemctl --user is-active "$service")" == "active" ]] || die "$service stopped during soak"
  [[ "$(systemctl --user show "$service" -p MainPID --value)" == "$service_pid" ]] || die "$service restarted during soak"
  authorize
  drive_result="$(drive_pulse)"
  jq -e --argjson drive "$drive" '.ok == true and .left == $drive and .right == $drive' \
    <<<"$drive_result" >/dev/null || die "low-speed pulse was not applied exactly"
  sleep "$pulse_sleep"

  stop_started_ns="$(date +%s%N)"
  zero="$(verified_stop)"
  stop_finished_ns="$(date +%s%N)"
  stop_latency_ns=$((stop_finished_ns - stop_started_ns))
  stop_latency_ms="$(awk -v ns="$stop_latency_ns" 'BEGIN {printf "%.3f", ns / 1000000}')"
  printf '%s\n' "$stop_latency_ns" >>"$latency_file"
  jq -e '.acknowledged == true' <<<"$zero" >/dev/null || die "verified stop was not acknowledged"
  telemetry="$(curl -fsS --max-time 2 "$base_url/telemetry")"
  jq -e '.left_cmd == 0 and .right_cmd == 0 and .estop == false' <<<"$telemetry" >/dev/null \
    || die "zero motion was not observed after verified stop"
  runtime_v2="$(curl -fsS --max-time 2 "$base_url/runtime-v2/status")"
  current_stop_sequence="$(jq -r '.controller.last_stop_receipt.applied_sequence' <<<"$runtime_v2")"
  (( current_stop_sequence > previous_stop_sequence )) || die "stop receipt sequence did not advance"
  jq -e '.controller.last_stop_receipt.verified_zero == true' <<<"$runtime_v2" >/dev/null \
    || die "controller owner did not verify zero"
  jq -nc --argjson pulse "$((pulses + 1))" --argjson latency_ms "$stop_latency_ms" \
    --argjson receipt "$(jq -c '.controller.last_stop_receipt' <<<"$runtime_v2")" \
    --argjson zero "$zero" \
    '{pulse:$pulse,stop_latency_ms:$latency_ms,controller_receipt:$receipt,adapter_zero:$zero}' \
    >>"$sample_file"
  previous_stop_sequence="$current_stop_sequence"
  pulses=$((pulses + 1))
  sleep "$settle_sleep"
done

authorize
before_estop_status="$(curl -fsS --max-time 2 "$base_url/runtime-v2/status")"
previous_estop_sequence="$(jq -r '.controller.last_estop_receipt.applied_sequence // 0' <<<"$before_estop_status")"
estop_started_ns="$(date +%s%N)"
curl -fsS --max-time 2 -X POST "$base_url/estop" >/dev/null
estop_finished_ns="$(date +%s%N)"
estop_latency_ns=$((estop_finished_ns - estop_started_ns))
estop_latency_ms="$(awk -v ns="$estop_latency_ns" 'BEGIN {printf "%.3f", ns / 1000000}')"
estop_status="$(curl -fsS --max-time 2 "$base_url/runtime-v2/status")"
estop_sequence="$(jq -r '.controller.last_estop_receipt.applied_sequence' <<<"$estop_status")"
(( estop_sequence > previous_estop_sequence && estop_sequence > previous_stop_sequence )) \
  || die "E-stop receipt sequence did not advance in order"
jq -e '.controller.estopped == true and .controller.last_estop_receipt.verified_zero == true' \
  <<<"$estop_status" >/dev/null || die "controller owner did not latch E-stop with verified zero"
telemetry="$(curl -fsS --max-time 2 "$base_url/telemetry")"
jq -e '.left_cmd == 0 and .right_cmd == 0 and .estop == true' <<<"$telemetry" >/dev/null \
  || die "E-stop zero motion was not observed"

reset_payload="$(jq -nc --arg token "$pilot_token" '{token:$token,approval:true}')"
curl -fsS --max-time 2 -X POST -H 'Content-Type: application/json' --data "$reset_payload" \
  "$base_url/estop/reset" >/dev/null
final_zero="$(verified_stop)"
jq -e '.acknowledged == true' <<<"$final_zero" >/dev/null || die "final verified zero was not acknowledged"
final_health="$(curl -fsS --max-time 2 "$base_url/health")"
final_cognition="$(curl -fsS --max-time 2 "$base_url/cognition/status")"
final_status="$(curl -fsS --max-time 2 "$base_url/runtime-v2/status")"
jq -e --arg backend "$backend" \
  '.ok == true and .accelerator.active == $backend and .accelerator.required == true' \
  <<<"$final_health" >/dev/null || die "backend health changed during soak"
jq -e --arg backend "$backend" \
  '.ok == true and .backend_status.active == $backend and .backend_status.degraded == false' \
  <<<"$final_cognition" >/dev/null || die "cognition backend degraded during soak"
jq -e \
  '.controller.connected == true and .controller.estopped == false and .controller.last_stop_receipt.verified_zero == true and .supervisor.faulted == false and .supervisor.evidence.healthy == true' \
  <<<"$final_status" >/dev/null || die "Runtime v2 authority degraded during soak"

final_write_failures="$(jq -r '.controller.metrics.write_failures' <<<"$final_status")"
final_ack_failures="$(jq -r '.supervisor.metrics.acknowledgement_failures' <<<"$final_status")"
final_faults="$(jq -r '.supervisor.metrics.faults' <<<"$final_status")"
final_evidence_failures="$(jq -r '.supervisor.metrics.evidence_failures' <<<"$final_status")"
(( final_write_failures == baseline_write_failures )) || die "controller write failure count increased"
(( final_ack_failures == baseline_ack_failures )) || die "acknowledgement failure count increased"
(( final_faults == baseline_faults )) || die "supervisor fault count increased"
(( final_evidence_failures == baseline_evidence_failures )) || die "evidence failure count increased"

sample_count="$(wc -l <"$latency_file" | tr -d ' ')"
(( sample_count > 0 )) || die "no stop samples were recorded"
p99_index=$(( (sample_count * 99 + 99) / 100 ))
p99_stop_ns="$(sort -n "$latency_file" | sed -n "${p99_index}p")"
p99_stop_ms="$(awk -v ns="$p99_stop_ns" 'BEGIN {printf "%.3f", ns / 1000000}')"
awk -v value="$p99_stop_ms" 'BEGIN {exit !(value <= 250)}' || die "verified-stop p99 exceeded 250 ms"
awk -v value="$estop_latency_ms" 'BEGIN {exit !(value <= 250)}' || die "E-stop acknowledgement exceeded 250 ms"

kill "$tegrastats_pid" 2>/dev/null || true
wait "$tegrastats_pid" 2>/dev/null || true
tegrastats_pid=""
tegrastats_samples="$(wc -l <"$tegrastats_file" | tr -d ' ')"
max_ram_mb="$(awk '{for (i=1;i<=NF;i++) if ($i=="RAM") {split($(i+1),parts,"/"); if (parts[1]>max) max=parts[1]}} END {print max+0}' "$tegrastats_file")"
max_gpu_temp_c="$(awk '{for (i=1;i<=NF;i++) if ($i ~ /^GPU@/) {value=$i; sub(/^GPU@/,"",value); sub(/C.*/,"",value); if (value>max) max=value}} END {printf "%.3f", max+0}' "$tegrastats_file")"
(( max_ram_mb < 3072 )) || die "RAM exceeded the 3072 MiB ceiling"
awk -v value="$max_gpu_temp_c" 'BEGIN {exit !(value < 80)}' || die "GPU temperature reached the 80 C ceiling"
tegrastats_sha256="$(sha256sum "$tegrastats_file" | awk '{print $1}')"

samples="$(jq -s '.' "$sample_file")"
finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
jq -n \
  --arg schema_version "leash.runtime-v2-hardware-soak.v1" \
  --arg started_at "$started_at" \
  --arg finished_at "$finished_at" \
  --arg backend "$backend" \
  --arg binary "$binary" \
  --arg binary_sha256 "$binary_sha256" \
  --arg service "$service" \
  --argjson service_pid "$service_pid" \
  --argjson duration_secs "$duration_secs" \
  --argjson pulses "$pulses" \
  --argjson drive "$drive" \
  --argjson pulse_ms "$pulse_ms" \
  --argjson settle_ms "$settle_ms" \
  --argjson p99_stop_ms "$p99_stop_ms" \
  --argjson estop_latency_ms "$estop_latency_ms" \
  --argjson initial_zero "$initial_zero" \
  --argjson final_zero "$final_zero" \
  --argjson health "$final_health" \
  --argjson cognition "$final_cognition" \
  --argjson runtime_v2 "$final_status" \
  --argjson samples "$samples" \
  --argjson tegrastats_samples "$tegrastats_samples" \
  --argjson max_ram_mb "$max_ram_mb" \
  --argjson max_gpu_temp_c "$max_gpu_temp_c" \
  --arg tegrastats_sha256 "$tegrastats_sha256" \
  '{schema_version:$schema_version,ok:true,started_at:$started_at,finished_at:$finished_at,backend:$backend,binary:{path:$binary,sha256:$binary_sha256},service:{name:$service,pid:$service_pid,restarted:false},motion:{duration_secs:$duration_secs,pulses:$pulses,normalized_drive:$drive,pulse_ms:$pulse_ms,settle_ms:$settle_ms},thresholds:{verified_stop_p99_ms:250,estop_ack_ms:250,max_ram_mb:3072,max_gpu_temp_c:80},results:{verified_stop_p99_ms:$p99_stop_ms,estop_ack_ms:$estop_latency_ms,zero_ack_timeouts:0,initial_zero:$initial_zero,final_zero:$final_zero,samples:$samples},health:($health|{ok,mode,role,profile,accelerator}),cognition:($cognition|{ok,backend_status,capabilities}),runtime_v2:$runtime_v2,resources:{tegrastats_samples:$tegrastats_samples,max_ram_mb:$max_ram_mb,max_gpu_temp_c:$max_gpu_temp_c,tegrastats_sha256:$tegrastats_sha256},operator_token_recorded:false}' \
  >"$output"
chmod 600 "$output" "$tegrastats_file"
printf '{"ok":true,"backend":"%s","pulses":%s,"verified_stop_p99_ms":%s,"estop_ack_ms":%s,"output":"%s"}\n' \
  "$backend" "$pulses" "$p99_stop_ms" "$estop_latency_ms" "$output"
