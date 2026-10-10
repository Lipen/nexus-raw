#!/bin/sh
# Refuse external project names in tracked sources: the tool is generic, and
# a leaked name ties it to one deployment and one org.
# `.dev/` is exempt: it never leaves the machine.
set -eu

# The names, space-separated. Extend the list as new deployments appear.
banned="arkui arkts panda openlab koala huawei bz-openlab"
hits=0

for name in $banned; do
  # -i: the casing varies (OpenLab, panda-sdk); word-ish boundary via the regex class.
  if grep -rIn -i -E "(^|[^a-z0-9])${name}([^a-z0-9]|$)" \
      --exclude-dir=.dev --exclude-dir=target --exclude-dir=node_modules \
      --exclude-dir=.git --exclude-dir=site --exclude-dir=.cache \
      . 2>/dev/null | grep -v "scripts/check-external-names.sh"; then
    echo "check-external-names: the name '$name' must not appear in tracked sources" >&2
    hits=1
  fi
done

if [ "$hits" -ne 0 ]; then
  echo "check-external-names: replace the names with neutral examples (nexus.example.com, my-alias)" >&2
  exit 1
fi
echo "check-external-names: clean"
