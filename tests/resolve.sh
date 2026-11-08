#!/bin/sh
set -u

resolver=${RESOLVER_UNDER_TEST:-$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)/resolve}
test_shell=${TEST_SHELL:-/bin/sh}
target=x86_64-unknown-linux-gnu
binary=develop-docs-check
version=0.1.0
other_version=0.2.0
asset=develop-tools-v$version-$target.tar.gz
root=$(mktemp -d "${TMPDIR:-/tmp}/develop-meta-resolve-test.XXXXXXXX") || exit 1
trap 'cleanup' 0

cleanup() {
  if [ -n "${server_pid:-}" ]; then
    kill "$server_pid" 2>/dev/null || :
    wait "$server_pid" 2>/dev/null || :
  fi
  for case_dir in "$root"/*; do
    [ -d "$case_dir" ] || continue
    rm -f "$case_dir"/repo/.develop-tools.json "$case_dir"/repo/.develop-tools-next.json \
      "$case_dir"/repo/.develop-tools-original.json "$case_dir"/repo/.config/develop-tools.json \
      "$case_dir"/releases/* "$case_dir"/payload/* \
      "$case_dir"/fakebin/* "$case_dir"/restricted/* "$case_dir"/local/* \
      "$case_dir"/cache/develop-tools/$version/$target/* \
      "$case_dir"/cache/develop-tools/$version/$target/.sha256 \
      "$case_dir"/cache/develop-tools/$other_version/$target/* \
      "$case_dir"/cache/develop-tools/$other_version/$target/.sha256 \
      "$case_dir"/cache/develop-tools/.manifest-$target-* \
      "$case_dir"/cache/develop-tools/$version/$target.lock/pid \
      "$case_dir"/state/mise/tracked-configs/* \
      "$case_dir"/curl.log "$case_dir"/tar.log "$case_dir"/run.log "$case_dir"/attempts \
      "$case_dir"/server.py "$case_dir"/server.log "$case_dir"/port "$case_dir"/leak \
      "$case_dir"/stdout "$case_dir"/stderr "$case_dir"/jq.log "$case_dir"/snapshot.log
    rmdir "$case_dir"/cache/develop-tools/$version/$target.lock.reap \
      "$case_dir"/cache/develop-tools/$version/$target.lock \
      "$case_dir"/cache/develop-tools/$version/$target \
      "$case_dir"/cache/develop-tools/$other_version/$target \
      "$case_dir"/cache/develop-tools/$other_version \
      "$case_dir"/cache/develop-tools/$version \
      "$case_dir"/cache/develop-tools "$case_dir"/cache \
      "$case_dir"/repo/.config "$case_dir"/repo "$case_dir"/releases "$case_dir"/payload \
      "$case_dir"/fakebin "$case_dir"/restricted "$case_dir"/local \
      "$case_dir"/state/mise/tracked-configs "$case_dir"/state/mise "$case_dir"/state \
      "$case_dir"/home "$case_dir" 2>/dev/null || :
  done
  rmdir "$root" 2>/dev/null || :
}

fail_test() { printf 'FAIL: %s\n' "$1" >&2; exit 1; }
assert_contains() { case "$1" in *"$2"*) ;; *) fail_test "expected '$2' in '$1'" ;; esac; }
real_command() {
  old_ifs=$IFS
  IFS=:
  for path_dir in $PATH; do
    case "$path_dir" in */mise/shims) continue ;; esac
    if [ -x "$path_dir/$1" ]; then
      printf '%s\n' "$path_dir/$1"
      IFS=$old_ifs
      return 0
    fi
  done
  IFS=$old_ifs
  return 1
}

pack_payload() {
  [ "$#" -gt 0 ] || set -- "$binary"
  tar -czf "$case_dir/releases/$asset" -C "$case_dir/payload" "$@" || exit 1
  if command -v sha256sum >/dev/null 2>&1; then
    digest=$(sha256sum "$case_dir/releases/$asset" | awk '{print $1}')
  else
    digest=$(shasum -a 256 "$case_dir/releases/$asset" | awk '{print $1}')
  fi
  printf '%s  %s\n' "$digest" "$asset" > "$case_dir/releases/SHA256SUMS"
  mkdir -p "$case_dir/repo/.config"
  printf '{"version":"%s","sha256":{"%s":"%s"}}\n' "$version" "$target" "$digest" > "$case_dir/repo/.config/develop-tools.json"
}

fixture() {
  case_dir=$root/$1
  mkdir -p "$case_dir/repo" "$case_dir/releases" "$case_dir/payload" \
    "$case_dir/fakebin" "$case_dir/cache" "$case_dir/home" || exit 1
  printf '#!/bin/sh\nprintf "ran:%%s\\n" "$*"\n' > "$case_dir/payload/$binary"
  chmod +x "$case_dir/payload/$binary"
  pack_payload
  cat > "$case_dir/fakebin/curl" <<'EOF'
#!/bin/sh
if [ "$1" = --help ]; then printf '%s\n' '--retry-all-errors'; exit 0; fi
safe_args=
skip=0
header=
for arg do
  if [ "$skip" -eq 1 ]; then
    case "$arg" in Authorization:*) header=$arg ;; esac
    skip=0
    continue
  fi
  case "$arg" in
    -H|--header) safe_args="$safe_args $arg [redacted]"; skip=1 ;;
    *) safe_args="$safe_args $arg" ;;
  esac
done
printf '%s\n' "$safe_args" >> "$CURL_LOG"
if [ "${FAKE_HTTP_STATUS:-}" = 401 ]; then printf '401'; exit 22; fi
if [ "${EXPECT_TOKEN+x}" = x ]; then
  if [ -n "$EXPECT_TOKEN" ]; then
    [ "$header" = "Authorization: Bearer $EXPECT_TOKEN" ] || exit 71
  else
    [ -z "$header" ] || exit 72
  fi
else
  [ -z "$header" ] || exit 72
fi
[ "${CURL_FAIL:-0}" = 0 ] || exit 22
[ "${CURL_DELAY:-0}" = 0 ] || /bin/sleep "$CURL_DELAY"
out=
prev=
for arg do
  case "$prev" in -o) out=$arg; prev=; continue ;; esac
  case "$arg" in -o) prev=-o ;; *) url=$arg ;; esac
done
case "$url" in
  */releases/tags/*)
    case "${FAKE_ASSETS:-unique}" in
      duplicate) printf '{"assets":[{"name":"%s","id":11},{"name":"%s","id":12}]}' "$asset" "$asset" > "$out" ;;
      missing) printf '{"assets":[]}' > "$out" ;;
      *) printf '{"assets":[{"name":"%s","id":11},{"name":"SHA256SUMS","id":12}]}' "$asset" > "$out" ;;
    esac
    exit 0 ;;
  */releases/assets/11) cp "$RELEASES/$asset" "$out"; exit $? ;;
  */releases/assets/12) cp "$RELEASES/SHA256SUMS" "$out"; exit $? ;;
esac
cp "$RELEASES/${url##*/}" "$out"
EOF
  cat > "$case_dir/fakebin/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) echo "${FAKE_OS:-Linux}" ;; -m) echo "${FAKE_ARCH:-x86_64}" ;; esac
EOF
  printf '#!/bin/sh\n/bin/sleep 0.02\n' > "$case_dir/fakebin/sleep"
  chmod +x "$case_dir/fakebin"/*
}

run_resolver() {
  mode=$1
  shift
  (cd "$case_dir/repo" || exit 1
    unset DEVELOP_TOOLS_BIN_DIR
    if [ "${LOCAL_BIN+x}" = x ]; then
      DEVELOP_TOOLS_BIN_DIR=$LOCAL_BIN
      export DEVELOP_TOOLS_BIN_DIR
    fi
    PATH="${RUN_PATH:-$case_dir/fakebin:$PATH}" XDG_CACHE_HOME="$case_dir/cache" \
    XDG_STATE_HOME="$case_dir/state" \
    RUN_LOG="$case_dir/run.log" HOME="$case_dir/home" RELEASES="$case_dir/releases" \
    CURL_LOG="$case_dir/curl.log" JQ_LOG="$case_dir/jq.log" \
    SNAPSHOT_LOG="$case_dir/snapshot.log" CURL_FAIL="${CURL_FAIL:-0}" \
    CURL_DELAY="${CURL_DELAY:-0}" FAKE_ASSETS="${FAKE_ASSETS:-unique}" asset="$asset" FAKE_OS="${FAKE_OS:-Linux}" \
    GITHUB_TOKEN="${TEST_GITHUB_TOKEN:-}" GH_TOKEN="${TEST_GH_TOKEN:-}" \
    EXPECT_TOKEN="${EXPECT_TOKEN:-}" \
    TEST_SERVER_PORT="${TEST_SERVER_PORT:-}" SERVER_DIR="$case_dir" REAL_CURL="${REAL_CURL:-}" \
    FAKE_HTTP_STATUS="${FAKE_HTTP_STATUS:-}" \
    FAKE_ARCH="${FAKE_ARCH:-x86_64}" \
    "$test_shell" "$resolver" "$mode" "$binary" "$@") \
    > "$case_dir/stdout" 2> "$case_dir/stderr"
  result=$?
  return "$result"
}

run_bin_dir() {
  mode=$1
  (cd "$case_dir/repo" || exit 1
    unset DEVELOP_TOOLS_BIN_DIR
    PATH="${RUN_PATH:-$case_dir/fakebin:$PATH}" XDG_CACHE_HOME="$case_dir/cache" \
    XDG_STATE_HOME="$case_dir/state" \
    RUN_LOG="$case_dir/run.log" HOME="$case_dir/home" RELEASES="$case_dir/releases" \
    CURL_LOG="$case_dir/curl.log" CURL_FAIL="${CURL_FAIL:-0}" \
    CURL_DELAY="${CURL_DELAY:-0}" FAKE_ASSETS="${FAKE_ASSETS:-unique}" asset="$asset" FAKE_OS="${FAKE_OS:-Linux}" \
    GITHUB_TOKEN="${TEST_GITHUB_TOKEN:-}" GH_TOKEN="${TEST_GH_TOKEN:-}" \
    EXPECT_TOKEN="${EXPECT_TOKEN:-}" \
    TEST_SERVER_PORT="${TEST_SERVER_PORT:-}" SERVER_DIR="$case_dir" REAL_CURL="${REAL_CURL:-}" \
    FAKE_HTTP_STATUS="${FAKE_HTTP_STATUS:-}" \
    FAKE_ARCH="${FAKE_ARCH:-x86_64}" \
    "$test_shell" "$resolver" bin-dir "$mode") \
    > "$case_dir/stdout" 2> "$case_dir/stderr"
  result=$?
  return "$result"
}

# run-heavy-check の payload 境界: resolver は先頭の -- も空引数も保持する。
binary=run-heavy-check
fixture heavy_transparency
cat > "$case_dir/payload/$binary" <<'EOF'
#!/bin/sh
printf 'started\n' > "$RUN_LOG"
for arg do printf '<%s>\n' "$arg"; done
cat
exit 37
EOF
pack_payload
printf '\000\377input\n' > "$case_dir/snapshot.log"
run_resolver ci -- command 'arg with space' '' -- < "$case_dir/snapshot.log"
[ "$result" -eq 37 ] || fail_test "resolver changed payload exit code ($result): $(cat "$case_dir/stderr")"
printf '<-->\n<command>\n<arg with space>\n<>\n<-->\n\000\377input\n' > "$case_dir/run.log"
cmp "$case_dir/run.log" "$case_dir/stdout" || fail_test 'resolver changed argv or binary stdin'
printf 'ok: run-heavy-check argv, stdin and exit code are transparent\n'

for failure in digest unpublished missing_binary; do
  fixture "heavy_$failure"
  cat > "$case_dir/payload/$binary" <<'EOF'
#!/bin/sh
printf 'started\n' > "$RUN_LOG"
EOF
  pack_payload
  case "$failure" in
    digest) printf '{"version":"%s","sha256":{"%s":"%064d"}}\n' "$version" "$target" 0 > "$case_dir/repo/.config/develop-tools.json" ;;
    unpublished) CURL_FAIL=1 ;;
    missing_binary)
      # 公開された旧版を模した、有効な digest だが runner を持たない archive。
      mv "$case_dir/payload/$binary" "$case_dir/payload/old-tool"
      pack_payload old-tool
      ;;
  esac
  run_resolver ci -- command
  unset CURL_FAIL
  [ "$result" -ne 0 ] || fail_test "$failure unexpectedly succeeded"
  [ ! -e "$case_dir/run.log" ] || fail_test "$failure started the payload"
done
printf 'ok: unavailable run-heavy-check never starts payload\n'
binary=develop-docs-check

# A killed resolver leaves this directory behind. The next invocation must reclaim it.
fixture stale_lock
lock=$case_dir/cache/develop-tools/$version/$target.lock
mkdir -p "$lock"
printf '99999999\n' > "$lock/pid"
run_resolver ci arg
[ "$result" -eq 0 ] || fail_test "stale lock was not reclaimed: $(cat "$case_dir/stderr")"
assert_contains "$(cat "$case_dir/stdout")" 'ran:arg'
printf 'ok: stale lock recovery (%s)\n' "$test_shell"

fixture empty_lock
lock=$case_dir/cache/develop-tools/$version/$target.lock
mkdir -p "$lock"
touch -t 200001010000 "$lock"
run_resolver ci
[ "$result" -eq 0 ] || fail_test "old empty lock was not reclaimed: $(cat "$case_dir/stderr")"
printf 'ok: old empty lock recovery\n'

fixture incomplete_lock
lock=$case_dir/cache/develop-tools/$version/$target.lock
mkdir -p "$lock"
: > "$lock/pid"
touch -t 200001010000 "$lock"
run_resolver ci
[ "$result" -eq 0 ] || fail_test "old incomplete lock was not reclaimed: $(cat "$case_dir/stderr")"
printf 'ok: old incomplete lock recovery\n'

fixture live_lock
lock=$case_dir/cache/develop-tools/$version/$target.lock
mkdir -p "$lock"
printf '%s\n' "$$" > "$lock/pid"
run_resolver pre-commit
[ "$result" -eq 0 ] || fail_test 'pre-commit failed on live lock timeout'
assert_contains "$(cat "$case_dir/stderr")" '取得できません'
run_resolver ci
[ "$result" -ne 0 ] || fail_test 'ci passed on live lock timeout'
assert_contains "$(cat "$case_dir/stderr")" '取得できません'
printf 'ok: lock timeout modes\n'

for format in text binary; do
  fixture "sums_$format"
  if [ "$format" = binary ]; then
    printf '%s *%s\n' "$digest" "$asset" > "$case_dir/releases/SHA256SUMS"
  fi
  run_resolver ci 'arg with space'
  [ "$result" -eq 0 ] || fail_test "SHA256SUMS $format was rejected: $(cat "$case_dir/stderr")"
  assert_contains "$(cat "$case_dir/stdout")" 'ran:arg with space'
  assert_contains "$(cat "$case_dir/curl.log")" '--retry 3 --retry-all-errors --connect-timeout 5 --max-time 30'
done
printf 'ok: SHA256SUMS formats and curl options\n'

fixture bin_dir
run_bin_dir ci
[ "$result" -eq 0 ] || fail_test "bin-dir download failed: $(cat "$case_dir/stderr")"
[ "$(cat "$case_dir/stdout")" = "$case_dir/cache/develop-tools/$version/$target" ] || fail_test 'bin-dir returned an unexpected path'
run_bin_dir ci
[ "$result" -eq 0 ] || fail_test "bin-dir cache hit failed: $(cat "$case_dir/stderr")"
[ "$(wc -l < "$case_dir/curl.log")" -eq 2 ] || fail_test 'bin-dir fetched the cache twice'
[ "$(cat "$case_dir/stdout")" = "$case_dir/cache/develop-tools/$version/$target" ] || fail_test 'bin-dir cache hit returned an unexpected path'
printf 'ok: verified bin-dir output without executing a tool\n'

fixture replace_cache
cache_dir=$case_dir/cache/develop-tools/$version/$target
mkdir -p "$cache_dir"
printf 'old\n' > "$cache_dir/$binary"
printf 'stale\n' > "$cache_dir/.sha256"
run_resolver ci
[ "$result" -eq 0 ] || fail_test "cache replacement failed: $(cat "$case_dir/stderr")"
assert_contains "$(cat "$case_dir/stdout")" 'ran:'
[ "$(cat "$cache_dir/.sha256")" = "$digest" ] || fail_test 'cache marker was not replaced'
printf 'ok: stale cache replacement\n'

fixture missing_cached_binary
cache_dir=$case_dir/cache/develop-tools/$version/$target
mkdir -p "$cache_dir"
printf '%s\n' "$digest" > "$cache_dir/.sha256"
run_resolver ci recovered
[ "$result" -eq 0 ] || fail_test "missing cached binary was not fetched: $(cat "$case_dir/stderr")"
assert_contains "$(cat "$case_dir/stdout")" 'ran:recovered'
[ "$(wc -l < "$case_dir/curl.log")" -eq 2 ] || fail_test 'missing cached binary did not trigger a fetch'
printf 'ok: missing cached binary is fetched again\n'

for absent in jq curl tar digest; do
  fixture "missing_$absent"
  mkdir "$case_dir/restricted"
  for required in uname mkdir mktemp cat cp tr date stat sleep rm rmdir mv awk; do
    if [ "$required" = uname ]; then
      ln -s "$case_dir/fakebin/uname" "$case_dir/restricted/uname"
    else
      ln -s "$(real_command "$required")" "$case_dir/restricted/$required"
    fi
  done
  [ "$absent" = jq ] || ln -s "$(real_command jq)" "$case_dir/restricted/jq"
  [ "$absent" = curl ] || ln -s "$case_dir/fakebin/curl" "$case_dir/restricted/curl"
  [ "$absent" = tar ] || ln -s "$(real_command tar)" "$case_dir/restricted/tar"
  if [ "$absent" != digest ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      ln -s "$(real_command sha256sum)" "$case_dir/restricted/sha256sum"
    else
      ln -s "$(real_command shasum)" "$case_dir/restricted/shasum"
    fi
  fi
  RUN_PATH=$case_dir/restricted
  run_resolver ci
  unset RUN_PATH
  [ "$result" -ne 0 ] || fail_test "missing $absent unexpectedly passed"
  case "$absent" in digest) expected='sha256sum または shasum' ;; *) expected="$absent" ;; esac
  assert_contains "$(cat "$case_dir/stderr")" "$expected"
done
printf 'ok: missing commands named\n'

fixture manifest_aba
cp "$case_dir/repo/.config/develop-tools.json" "$case_dir/repo/.develop-tools-original.json"
printf '{"version":"%s","sha256":{"%s":"%064d"}}\n' "$other_version" "$target" 0 > "$case_dir/repo/.develop-tools-next.json"
if command -v sha256sum >/dev/null 2>&1; then
  original_digest=$(sha256sum "$case_dir/repo/.config/develop-tools.json" | awk '{print $1}')
else
  original_digest=$(shasum -a 256 "$case_dir/repo/.config/develop-tools.json" | awk '{print $1}')
fi
for cache_version in "$version" "$other_version"; do
  cache_dir=$case_dir/cache/develop-tools/$cache_version/$target
  mkdir -p "$cache_dir"
  cp "$case_dir/payload/$binary" "$cache_dir/$binary"
  printf '%s\n' "$digest" > "$cache_dir/.sha256"
done
# Each jq call changes the source manifest before forwarding its original arguments.
# shellcheck disable=SC2016
printf '#!/bin/sh\nif [ ! -e "$JQ_LOG" ]; then\n  cp .develop-tools-next.json .config/develop-tools.json\nelse\n  cp .develop-tools-original.json .config/develop-tools.json\nfi\nprintf "jq\\n" >> "$JQ_LOG"\nfor arg do snapshot=$arg; done\nprintf "%%s\\n" "$snapshot" >> "$SNAPSHOT_LOG"\nexec "%s" "$@"\n' "$(real_command jq)" > "$case_dir/fakebin/jq"
chmod +x "$case_dir/fakebin/jq"
run_resolver ci aba
[ "$result" -eq 0 ] || fail_test "manifest ABA setup failed: $(cat "$case_dir/stderr")"
[ "$(wc -l < "$case_dir/jq.log")" -eq 2 ] || fail_test 'manifest ABA did not call jq twice'
read -r snapshot_path < "$case_dir/snapshot.log"
[ "$(wc -l < "$case_dir/snapshot.log")" -eq 2 ] || fail_test 'manifest ABA did not record both jq sources'
[ "$(sed -n '2p' "$case_dir/snapshot.log")" = "$snapshot_path" ] || fail_test 'manifest ABA used different jq sources'
[ ! -e "$snapshot_path" ] || fail_test 'manifest snapshot survived the resolver'
[ "$(cat "$case_dir/repo/.config/develop-tools.json")" = "$(cat "$case_dir/repo/.develop-tools-original.json")" ] || fail_test 'manifest ABA did not restore the source'
index=$case_dir/cache/develop-tools/.manifest-$target-$original_digest
[ "$(cat "$index")" = "$original_digest $version $digest" ] || fail_test 'manifest ABA mixed the index with another version'
printf 'ok: manifest ABA keeps digest and jq values on one snapshot\n'

fixture manifest_race
cp "$case_dir/repo/.config/develop-tools.json" "$case_dir/repo/.develop-tools-next.json"
printf '{"version":"%s","sha256":{"%s":"%064d"}}\n' "$version" "$target" 0 > "$case_dir/repo/.config/develop-tools.json"
cache_dir=$case_dir/cache/develop-tools/$version/$target
mkdir -p "$cache_dir"
cp "$case_dir/payload/$binary" "$cache_dir/$binary"
printf '%s\n' "$digest" > "$cache_dir/.sha256"
# The first jq invocation replaces the manifest before reading it.
# shellcheck disable=SC2016
printf '#!/bin/sh\nif [ ! -e "$JQ_LOG" ]; then\n  cp .develop-tools-next.json .config/develop-tools.json\nfi\nprintf "jq\\n" >> "$JQ_LOG"\nexec "%s" "$@"\n' "$(real_command jq)" > "$case_dir/fakebin/jq"
chmod +x "$case_dir/fakebin/jq"
run_resolver ci raced
[ "$result" -ne 0 ] || fail_test 'manifest replacement escaped the snapshot check'
assert_contains "$(cat "$case_dir/stderr")" 'sha256 が一致しません'
[ "$(wc -l < "$case_dir/jq.log")" -eq 2 ] || fail_test 'manifest race did not call jq twice'
for index in "$case_dir"/cache/develop-tools/.manifest-"$target"-*; do
  [ ! -e "$index" ] || fail_test 'manifest race wrote an index for stale bytes'
done
printf '{"version":"%s","sha256":{"%s":"%064d"}}\n' "$version" "$target" 0 > "$case_dir/repo/.config/develop-tools.json"
run_resolver ci stale
[ "$result" -ne 0 ] || fail_test 'restored manifest executed a binary with a different digest'
assert_contains "$(cat "$case_dir/stderr")" 'sha256 が一致しません'
printf 'ok: manifest replacement during jq leaves no stale index\n'

fixture cache_hit
run_resolver ci first
[ "$result" -eq 0 ] || fail_test 'initial cache install failed'
if command -v sha256sum >/dev/null 2>&1; then
  manifest_digest=$(sha256sum "$case_dir/repo/.config/develop-tools.json" | awk '{print $1}')
else
  manifest_digest=$(shasum -a 256 "$case_dir/repo/.config/develop-tools.json" | awk '{print $1}')
fi
index=$case_dir/cache/develop-tools/.manifest-$target-$manifest_digest
[ -f "$index" ] || fail_test 'verified cache did not write a manifest index'
# The generated script reads JQ_LOG and forwards its arguments.
# shellcheck disable=SC2016
printf '#!/bin/sh\nprintf "jq\\n" >> "$JQ_LOG"\nexec "%s" "$@"\n' "$(real_command jq)" > "$case_dir/fakebin/jq"
chmod +x "$case_dir/fakebin/jq"
run_resolver ci second
[ "$result" -eq 0 ] || fail_test 'cache hit failed'
[ "$(wc -l < "$case_dir/curl.log")" -eq 2 ] || fail_test 'cache hit fetched again'
[ ! -e "$case_dir/jq.log" ] || fail_test 'manifest index hit called jq'
assert_contains "$(cat "$case_dir/stdout")" 'ran:second'
cp "$case_dir/repo/.config/develop-tools.json" "$case_dir/repo/.develop-tools-next.json"
printf '{ "version":"%s", "sha256":{"%s":"%s"}}\n' "$version" "$target" "$digest" > "$case_dir/repo/.config/develop-tools.json"
run_resolver ci rewritten
[ "$result" -eq 0 ] || fail_test 'rewritten manifest with the same digest was rejected'
[ "$(wc -l < "$case_dir/jq.log")" -eq 2 ] || fail_test 'changed manifest did not call jq'
[ "$(wc -l < "$case_dir/curl.log")" -eq 2 ] || fail_test 'same verified digest fetched again'
run_resolver ci rewritten_again
[ "$result" -eq 0 ] || fail_test 'rewritten manifest missed its index'
[ "$(wc -l < "$case_dir/jq.log")" -eq 2 ] || fail_test 'rewritten manifest called jq twice'
cp "$case_dir/repo/.develop-tools-next.json" "$case_dir/repo/.config/develop-tools.json"
run_resolver ci restored
[ "$result" -eq 0 ] || fail_test 'restored manifest was rejected'
[ "$(wc -l < "$case_dir/jq.log")" -eq 2 ] || fail_test 'restored manifest missed its index'
printf '{"version":"%s","sha256":{"%s":"%064d"}}\n' "$version" "$target" 0 > "$case_dir/repo/.config/develop-tools.json"
run_resolver ci
[ "$result" -ne 0 ] || fail_test 'changed manifest digest was accepted'
[ "$(wc -l < "$case_dir/jq.log")" -eq 4 ] || fail_test 'changed digest did not return to jq'
[ "$(wc -l < "$case_dir/curl.log")" -eq 4 ] || fail_test 'changed digest did not return to download'
assert_contains "$(cat "$case_dir/stderr")" 'sha256 が一致しません'
[ "$(cat "$case_dir/cache/develop-tools/$version/$target/.sha256")" = "$digest" ] || fail_test 'mismatch replaced verified cache'
printf 'ok: manifest index hit and changed manifest fallback\n'

for mismatch in manifest sums; do
  fixture "mismatch_$mismatch"
  if [ "$mismatch" = manifest ]; then
    printf '{"version":"%s","sha256":{"%s":"%064d"}}\n' "$version" "$target" 0 > "$case_dir/repo/.config/develop-tools.json"
  else
    printf '%064d  %s\n' 0 "$asset" > "$case_dir/releases/SHA256SUMS"
  fi
  run_resolver pre-commit
  [ "$result" -ne 0 ] || fail_test "$mismatch digest was accepted"
  assert_contains "$(cat "$case_dir/stderr")" 'sha256 が一致しません'
  [ ! -e "$case_dir/cache/develop-tools/$version/$target" ] || fail_test 'mismatched asset was extracted'
done
printf 'ok: both digest checks fail closed\n'

for mode_name in pre-commit ci pre-push release; do
  fixture "download_$mode_name"
  CURL_FAIL=1
  run_resolver "$mode_name"
  unset CURL_FAIL
  if [ "$mode_name" = pre-commit ]; then
    [ "$result" -eq 0 ] || fail_test 'pre-commit download failure was fatal'
  else
    [ "$result" -ne 0 ] || fail_test "$mode_name download failure passed"
  fi
  assert_contains "$(cat "$case_dir/stderr")" '取得できません'
done
printf 'ok: download failure modes\n'

fixture manifest_new_only
run_resolver ci
[ "$result" -eq 0 ] || fail_test "new-only manifest failed: $(cat "$case_dir/stderr")"
assert_contains "$(cat "$case_dir/stdout")" 'ran:'
printf 'ok: manifest new location only\n'

fixture manifest_legacy_only
mv "$case_dir/repo/.config/develop-tools.json" "$case_dir/repo/.develop-tools.json"
run_resolver ci
[ "$result" -ne 0 ] || fail_test 'legacy-only manifest unexpectedly passed'
assert_contains "$(cat "$case_dir/stderr")" '旧い置き場です'
assert_contains "$(cat "$case_dir/stderr")" '.config/develop-tools.json へ移す'
printf 'ok: manifest legacy location only fails\n'

fixture manifest_both
cp "$case_dir/repo/.config/develop-tools.json" "$case_dir/repo/.develop-tools.json"
run_resolver ci
[ "$result" -ne 0 ] || fail_test 'both manifests unexpectedly passed'
assert_contains "$(cat "$case_dir/stderr")" '.config/develop-tools.json と .develop-tools.json の両方があります'
printf 'ok: both manifests fail naming both paths\n'

fixture local_binary
mkdir "$case_dir/local"
printf '#!/bin/sh\nprintf "local:%%s\\n" "$*"\n' > "$case_dir/local/$binary"
chmod +x "$case_dir/local/$binary"
LOCAL_BIN=$case_dir/local
run_resolver ci 'arg with space'
[ "$result" -eq 0 ] || fail_test 'local binary failed'
assert_contains "$(cat "$case_dir/stdout")" 'local:arg with space'
[ ! -e "$case_dir/curl.log" ] || fail_test 'local binary fetched asset'
LOCAL_BIN=$case_dir/missing
run_resolver pre-commit
[ "$result" -ne 0 ] || fail_test 'missing local binary passed'
unset LOCAL_BIN
printf 'ok: explicit local binary\n'

fixture unsupported_target
FAKE_OS=Windows_NT
run_resolver pre-commit
[ "$result" -eq 0 ] || fail_test 'unsupported target stopped pre-commit'
run_resolver ci
[ "$result" -ne 0 ] || fail_test 'unsupported target passed ci'
unset FAKE_OS
[ ! -e "$case_dir/curl.log" ] || fail_test 'unsupported target fetched asset'
printf 'ok: unsupported target modes\n'

fixture parallel
# uutils 0.8.0 can return success after losing mkdir's creation race.
# Reproduce that result deterministically; only an actual creation emits -v output.
# shellcheck disable=SC2016
printf '#!/bin/sh\n"%s" "$@" && exit 0\nfor directory do :; done\n[ -d "$directory" ]\n' "$(real_command mkdir)" > "$case_dir/fakebin/mkdir"
chmod +x "$case_dir/fakebin/mkdir"
CURL_DELAY=0.2
run_resolver ci parallel &
first=$!
run_resolver ci parallel &
second=$!
wait "$first" || fail_test 'first parallel invocation failed'
wait "$second" || fail_test 'second parallel invocation failed'
unset CURL_DELAY
[ "$(wc -l < "$case_dir/curl.log")" -eq 2 ] || fail_test 'parallel calls fetched twice'
printf 'ok: parallel cache install\n'

fixture parallel_stale_lock
lock=$case_dir/cache/develop-tools/$version/$target.lock
mkdir -p "$lock"
printf '99999999\n' > "$lock/pid"
CURL_DELAY=0.2
run_resolver ci parallel &
first=$!
run_resolver ci parallel &
second=$!
wait "$first" || fail_test 'first stale-lock invocation failed'
wait "$second" || fail_test 'second stale-lock invocation failed'
unset CURL_DELAY
[ "$(wc -l < "$case_dir/curl.log")" -eq 2 ] || fail_test 'stale-lock callers fetched twice'
printf 'ok: parallel stale lock recovery\n'

for source in github gh anonymous; do
  fixture "auth_$source"
  case "$source" in
    github) TEST_GITHUB_TOKEN=fake-github-token TEST_GH_TOKEN=fake-gh-token EXPECT_TOKEN=$TEST_GITHUB_TOKEN ;;
    gh) TEST_GITHUB_TOKEN='' TEST_GH_TOKEN=fake-gh-token EXPECT_TOKEN=$TEST_GH_TOKEN ;;
    anonymous) TEST_GITHUB_TOKEN='' TEST_GH_TOKEN='' EXPECT_TOKEN='' ;;
  esac
  run_resolver ci
  [ "$result" -eq 0 ] || fail_test "$source download failed: $(cat "$case_dir/stderr")"
  if [ "$source" = anonymous ]; then
    [ "$(wc -l < "$case_dir/curl.log")" -eq 2 ] || fail_test 'anonymous did not use two public URLs'
    ! grep -Fq '/releases/tags/' "$case_dir/curl.log" || fail_test 'anonymous used API'
  else
    [ "$(wc -l < "$case_dir/curl.log")" -eq 3 ] || fail_test "$source did not fetch metadata and both assets"
    grep -Fq '/releases/tags/' "$case_dir/curl.log" || fail_test "$source did not use API"
  fi
done
unset TEST_GITHUB_TOKEN TEST_GH_TOKEN EXPECT_TOKEN
printf 'ok: token selection and anonymous fallback\n'

fixture auth_rejected
TEST_GITHUB_TOKEN=fake-token EXPECT_TOKEN=fake-token FAKE_HTTP_STATUS=401
run_resolver pre-commit
[ "$result" -ne 0 ] || fail_test 'authenticated 401 skipped pre-commit'
assert_contains "$(cat "$case_dir/stderr")" '認証が拒否されました'
! grep -Fq 'https://github.com/' "$case_dir/curl.log" || fail_test '401 downgraded to public URL'
unset TEST_GITHUB_TOKEN EXPECT_TOKEN FAKE_HTTP_STATUS
printf 'ok: authenticated rejection fails closed in pre-commit\n'

for condition in duplicate missing; do
  fixture "auth_$condition"
  TEST_GITHUB_TOKEN=fake-token EXPECT_TOKEN=fake-token FAKE_ASSETS=$condition
  run_resolver ci
  [ "$result" -ne 0 ] || fail_test "$condition metadata was accepted"
  [ ! -e "$case_dir/cache/develop-tools/$version/$target/.sha256" ] || fail_test "$condition was cached"
  ! grep -Fq 'https://github.com/' "$case_dir/curl.log" || fail_test "$condition downgraded to public URL"
done
unset TEST_GITHUB_TOKEN EXPECT_TOKEN FAKE_ASSETS
printf 'ok: metadata asset uniqueness and no downgrade\n'

fixture auth_checksum
TEST_GITHUB_TOKEN=fake-token EXPECT_TOKEN=fake-token
printf '%064d  %s\n' 0 "$asset" > "$case_dir/releases/SHA256SUMS"
run_resolver ci
[ "$result" -ne 0 ] || fail_test 'authenticated checksum mismatch passed'
pack_payload
printf '{"version":"%s","sha256":{"%s":"%064d"}}\n' "$version" "$target" 0 > "$case_dir/repo/.config/develop-tools.json"
run_resolver ci
[ "$result" -ne 0 ] || fail_test 'authenticated manifest mismatch passed'
unset TEST_GITHUB_TOKEN EXPECT_TOKEN
printf 'ok: authenticated SHA256SUMS and manifest verification\n'

fixture auth_http
cat > "$case_dir/server.py" <<'PY'
import http.server
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
asset = sys.argv[2]

class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_args):
        pass

    def do_GET(self):
        if self.path.endswith('/releases/tags/v0.1.0'):
            body = json.dumps({'assets': [{'name': asset, 'id': 11}, {'name': 'SHA256SUMS', 'id': 12}]}).encode()
        elif self.path.endswith('/releases/assets/11'):
            attempts = root / 'attempts'
            count = int(attempts.read_text()) + 1 if attempts.exists() else 1
            attempts.write_text(str(count))
            if count == 1:
                self.send_error(503)
                return
            self.send_response(302)
            self.send_header('Location', f'http://localhost:{self.server.server_port}/blob')
            self.end_headers()
            return
        elif self.path.endswith('/releases/assets/12'):
            body = (root / 'releases' / 'SHA256SUMS').read_bytes()
        elif self.path == '/blob':
            if self.headers.get('Authorization'):
                (root / 'leak').write_text('authorization forwarded')
                self.send_error(400)
                return
            body = (root / 'releases' / asset).read_bytes()
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
(root / 'port').write_text(str(server.server_port))
server.serve_forever()
PY
cat > "$case_dir/fakebin/curl" <<'BASH'
#!/usr/bin/env bash
if [[ $1 == --help ]]; then printf '%s\n' '--retry-all-errors'; exit 0; fi
args=()
while (($#)); do
  case "$1" in
    --proto|--proto-redir) shift 2 ;;
    https://api.github.com/*) args+=("http://127.0.0.1:${TEST_SERVER_PORT}/${1#https://api.github.com/}"); shift ;;
    *) args+=("$1"); shift ;;
  esac
done
exec "$REAL_CURL" "${args[@]}"
BASH
chmod +x "$case_dir/fakebin/curl"
python3 -u "$case_dir/server.py" "$case_dir" "$asset" > "$case_dir/server.log" 2>&1 &
server_pid=$!
counter=0
until [ -s "$case_dir/port" ]; do
  counter=$((counter + 1))
  [ "$counter" -lt 100 ] || fail_test 'fake server did not start'
  /bin/sleep 0.02
done
TEST_SERVER_PORT=$(cat "$case_dir/port")
REAL_CURL=$(real_command curl)
TEST_GITHUB_TOKEN=fake-token
run_resolver ci
[ "$result" -eq 0 ] || fail_test "redirect or retry failed: $(cat "$case_dir/stderr")"
[ "$(cat "$case_dir/attempts")" -eq 2 ] || fail_test 'HTTP 503 was not retried once'
[ ! -e "$case_dir/leak" ] || fail_test 'Authorization crossed redirect hosts'
kill "$server_pid"
wait "$server_pid" 2>/dev/null || :
server_pid=
unset TEST_SERVER_PORT REAL_CURL TEST_GITHUB_TOKEN
printf 'ok: real curl retry and cross-host Authorization stripping\n'

cleanup
trap - 0
[ ! -e "$root" ] || fail_test 'test temporary directory was not removed'
printf 'ok: test temporary directory cleanup\n'
