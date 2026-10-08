#!/bin/sh
set -eu
if [ "$(/usr/bin/id -u)" != 0 ]; then exec /usr/bin/sudo /bin/sh "$0"; fi
printf '%s\n' 'Removing OLC local USB networking. Quit OLC before using this command.'
/bin/launchctl bootout system/org.onelibrarycompanion.networking 2>/dev/null || true
/bin/rm -f /Library/LaunchDaemons/org.onelibrarycompanion.networking.plist /Library/PrivilegedHelperTools/org.onelibrarycompanion.networking '/Library/Application Support/OneLibraryCompanion/Networking/host.cdhash'
/bin/rmdir '/Library/Application Support/OneLibraryCompanion/Networking' 2>/dev/null || true
printf '%s\n' 'Networking component removed. Your application, music and saved sets were retained.'
