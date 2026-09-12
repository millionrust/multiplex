#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
STRUCTURAL=0
case "${1:-}" in
  --structural) STRUCTURAL=1 ;;
  "") ;;
  *) echo "usage: $0 [--structural]" >&2; exit 2 ;;
esac

cd "$ROOT"
export ANDROID_HOME=${ANDROID_HOME:-"$HOME/Library/Android/sdk"}

require_pattern() {
  pattern=$1
  path=$2
  label=$3
  grep -q "$pattern" "$path" || {
    echo "$label is missing from $path" >&2
    exit 1
  }
}

for path in \
  app/src/main/java/com/termirust/mobile/MainActivity.kt \
  app/src/main/java/com/termirust/mobile/ui/UnifiedMobileApp.kt \
  app/src/main/java/com/termirust/mobile/ssh/DirectSshSessionClient.kt \
  app/src/main/java/com/termirust/mobile/controller/ControllerApp.kt \
  app/src/main/java/com/termirust/mobile/controller/ControllerRemoteRoute.kt \
  app/src/main/java/com/termirust/mobile/controller/AndroidControllerRouteCoordinator.kt \
  app/src/main/java/com/termirust/mobile/controller/AndroidControllerRouteConnections.kt \
  app/src/main/java/com/termirust/mobile/controller/ControllerRemoteRouteConfiguration.kt; do
  [ -f "$path" ] || { echo "unified route source is missing: $path" >&2; exit 1; }
done

if grep -Eq 'flavorDimensions|productFlavors|applicationIdSuffix|legacyDirectSsh' app/build.gradle.kts; then
  echo "product flavors or compatibility-app metadata remain" >&2
  exit 1
fi
if [ -d app/src/controller ] || [ -d app/src/legacyDirectSsh ] || \
  [ -d app/src/testController ] || [ -d app/src/testLegacyDirectSsh ]; then
  echo "flavor-specific source sets remain" >&2
  exit 1
fi
require_pattern 'MobileRootDestination.CONNECTIONS' \
  app/src/main/java/com/termirust/mobile/ui/UnifiedMobileApp.kt "Connections destination"
require_pattern 'MobileRootDestination.DEVICES' \
  app/src/main/java/com/termirust/mobile/ui/UnifiedMobileApp.kt "Devices destination"
require_pattern 'Direct SSH' app/src/main/java/com/termirust/mobile/ui/TermirustApp.kt \
  "direct SSH route"
require_pattern 'DEVICE_SESSION("device_session")' \
  app/src/main/java/com/termirust/mobile/controller/MobileCrossRouteAcceptance.kt \
  "Device Session acceptance route"
require_pattern '<string name="previous_sessions">' app/src/main/res/values/strings.xml \
  "Sessions UI resource"
require_pattern 'implementation("com.hierynomus:sshj:0.39.0")' app/build.gradle.kts \
  "SSHJ runtime dependency"
require_pattern 'termirust-mobile-secrets' \
  app/src/main/java/com/termirust/mobile/security/KeystoreSecretStore.kt \
  "mobile secret-store namespace"
require_pattern 'termirust-controller-device-v1' \
  app/src/main/java/com/termirust/mobile/controller/ControllerSecureBlobStore.kt \
  "Controller secure-blob namespace"
require_pattern 'Routes never switch automatically' app/src/main/res/values/strings.xml \
  "explicit route-switching guidance"
require_pattern 'ControllerRouteCredentialStore' \
  app/src/main/java/com/termirust/mobile/controller/ControllerRemoteRouteConfiguration.kt \
  "Controller route credential store"
require_pattern 'routeConnections.disconnect' \
  app/src/main/java/com/termirust/mobile/controller/ControllerViewModel.kt \
  "Controller route disconnect lifecycle"

for abi in arm64-v8a armeabi-v7a x86 x86_64; do
  [ -f "app/src/main/jniLibs/$abi/libtermirust_controller_bindings.so" ] || {
    echo "Controller JNI library missing for $abi" >&2; exit 1;
  }
  [ -f "app/src/main/jniLibs/$abi/libtermirust_mobile_ffi.so" ] || {
    echo "direct SSH crypto JNI library missing for $abi" >&2; exit 1;
  }
done

REPORT=$(mktemp)
trap 'rm -f "$REPORT"' EXIT HUP INT TERM
./gradlew :app:dependencies --configuration debugRuntimeClasspath --console=plain >"$REPORT"
grep -Eiq 'sshj' "$REPORT" || { echo "unified runtime is missing SSHJ" >&2; exit 1; }

if [ "$STRUCTURAL" -eq 0 ]; then
  ./gradlew testDebugUnitTest assembleDebug --console=plain
  APK=app/build/outputs/apk/debug/app-debug.apk
  [ -f "$APK" ] || { echo "unified debug APK was not produced" >&2; exit 1; }
  for library in libtermirust_controller_bindings.so libtermirust_mobile_ffi.so; do
    COUNT=$(unzip -Z1 "$APK" | grep -c "/$library$")
    [ "$COUNT" -eq 4 ] || {
      echo "$library must be packaged once for each supported ABI" >&2
      exit 1
    }
  done
fi

echo "unified Android application contains Connections and Devices in one APK"
