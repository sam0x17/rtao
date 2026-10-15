#!/usr/bin/env bash
# Shared byte-exact notice staging for release builders.
source "$(dirname "${BASH_SOURCE[0]}")/pins.env"
rtao_license_source_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
rtao_root_cert_license_source="$rtao_license_source_root/docs/licenses/webpki-root-certs-LICENSE"
rtao_unicode_license_source="$rtao_license_source_root/docs/licenses/Unicode-DFS-2016.txt"
rtao_release_license_names=(LICENSE-MIT LICENSE-APACHE NOTICE LICENSE-PSF webpki-root-certs-LICENSE Unicode-DFS-2016.txt)
rtao_release_license_sources=(LICENSE-MIT LICENSE-APACHE NOTICE crates/rtao-primitives/LICENSE-PSF docs/licenses/webpki-root-certs-LICENSE docs/licenses/Unicode-DFS-2016.txt)

# NOTICE changes with the source tag/imported pins. Its expected identity comes
# from this checkout, rather than an immutable pin in pins.env.
rtao_notice_identity() {
  local path="$rtao_license_source_root/NOTICE"
  local size digest
  test -f "$path" && test ! -L "$path" || return 1
  size=$(stat -c %s -- "$path" 2>/dev/null) || size=$(stat -f %z "$path") || return 1
  test "$size" -gt 0 && test "$size" -le 1048576 || return 1
  if command -v sha256sum >/dev/null 2>&1; then
    digest=$(sha256sum -- "$path") || return 1
  else
    digest=$(shasum -a 256 -- "$path") || return 1
  fi
  rtao_notice_size=$size
  rtao_notice_sha256=${digest%% *}
}

rtao_release_license_identity() {
  case "${1:?notice name}" in
    LICENSE-MIT) rtao_license_size=$RTAO_MIT_LICENSE_SIZE; rtao_license_sha256=$RTAO_MIT_LICENSE_SHA256 ;;
    LICENSE-APACHE) rtao_license_size=$RTAO_APACHE_LICENSE_SIZE; rtao_license_sha256=$RTAO_APACHE_LICENSE_SHA256 ;;
    NOTICE)
      rtao_notice_identity || return 1
      rtao_license_size=$rtao_notice_size; rtao_license_sha256=$rtao_notice_sha256 ;;
    LICENSE-PSF) rtao_license_size=$RTAO_PSF_LICENSE_SIZE; rtao_license_sha256=$RTAO_PSF_LICENSE_SHA256 ;;
    webpki-root-certs-LICENSE) rtao_license_size=$RTAO_ROOT_CERT_LICENSE_SIZE; rtao_license_sha256=$RTAO_ROOT_CERT_LICENSE_SHA256 ;;
    Unicode-DFS-2016.txt) rtao_license_size=$RTAO_UNICODE_LICENSE_SIZE; rtao_license_sha256=$RTAO_UNICODE_LICENSE_SHA256 ;;
    *) return 1 ;;
  esac
}

rtao_verify_license() {
  local path=${1:?license path}
  local size=${2:?pinned license size}
  local sha256=${3:?pinned license hash}
  local actual actual_size
  if ! test -f "$path" || test -L "$path"; then
    printf 'license must be a regular file: %s\n' "$path" >&2
    return 1
  fi
  actual_size=$(stat -c %s -- "$path" 2>/dev/null) || actual_size=$(stat -f %z "$path") || return 1
  if test "$actual_size" != "$size"; then
    printf 'license size differs from its pinned text: %s\n' "$path" >&2
    return 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum -- "$path") || return 1
  else
    actual=$(shasum -a 256 -- "$path") || return 1
  fi
  if test "${actual%% *}" != "$sha256"; then
    printf 'license hash differs from its pinned text: %s\n' "$path" >&2
    return 1
  fi
}

rtao_verify_root_cert_license() {
  rtao_verify_license "${1:?root-cert license path}" "$RTAO_ROOT_CERT_LICENSE_SIZE" "$RTAO_ROOT_CERT_LICENSE_SHA256"
}

rtao_verify_unicode_license() {
  rtao_verify_license "${1:?Unicode license path}" "$RTAO_UNICODE_LICENSE_SIZE" "$RTAO_UNICODE_LICENSE_SHA256"
}

rtao_copy_license() {
  local directory=${1:?release asset directory}
  local source=${2:?source license path}
  local size=${3:?pinned license size}
  local sha256=${4:?pinned license hash}
  local destination="$directory/${source##*/}"
  rtao_verify_license "$source" "$size" "$sha256" || return 1
  test -d "$directory" && test ! -L "$directory" || return 1
  if test -e "$destination" || test -L "$destination"; then
    rtao_verify_license "$destination" "$size" "$sha256" || return 1
    cmp "$source" "$destination"
  else
    cp "$source" "$destination" || return 1
    rtao_verify_license "$destination" "$size" "$sha256"
  fi
}

rtao_copy_root_cert_license() {
  rtao_copy_license "${1:?release asset directory}" "$rtao_root_cert_license_source" "$RTAO_ROOT_CERT_LICENSE_SIZE" "$RTAO_ROOT_CERT_LICENSE_SHA256"
}

rtao_copy_unicode_license() {
  rtao_copy_license "${1:?release asset directory}" "$rtao_unicode_license_source" "$RTAO_UNICODE_LICENSE_SIZE" "$RTAO_UNICODE_LICENSE_SHA256"
}

rtao_copy_release_licenses() {
  local directory=${1:?release asset directory}
  local index name source
  for index in "${!rtao_release_license_names[@]}"; do
    name=${rtao_release_license_names[$index]}
    source="$rtao_license_source_root/${rtao_release_license_sources[$index]}"
    rtao_release_license_identity "$name" || return 1
    rtao_copy_license "$directory" "$source" "$rtao_license_size" "$rtao_license_sha256" || return 1
  done
}

rtao_verify_release_licenses() {
  local directory=${1:?release asset directory}
  local index name source
  test -d "$directory" && test ! -L "$directory" || return 1
  for index in "${!rtao_release_license_names[@]}"; do
    name=${rtao_release_license_names[$index]}
    source="$rtao_license_source_root/${rtao_release_license_sources[$index]}"
    rtao_release_license_identity "$name" || return 1
    rtao_verify_license "$source" "$rtao_license_size" "$rtao_license_sha256" || return 1
    rtao_verify_license "$directory/$name" "$rtao_license_size" "$rtao_license_sha256" || return 1
    cmp "$source" "$directory/$name" || return 1
  done
}
