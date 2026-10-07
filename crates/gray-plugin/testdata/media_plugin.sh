#!/bin/sh
# Protocol-1.3 fixture: `tool/call` replies carry `images` and `media`
# next to `content`. The second image is malformed (no data) and must be
# dropped without failing the result.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"name":"media","version":"0.1.0","protocol":"1.3","tools":[{"name":"shot","description":"x","parameters":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *plugin/tools*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"tools":[{"name":"shot","description":"x","parameters":{"type":"object","properties":{}}}]}}\n' "$id"
      ;;
    *tool/call*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '{"id":%s,"result":{"content":"here","images":[{"mime":"image/png","data_base64":"iVBORw0KGgo="},{"mime":"image/png"}],"media":[{"mime":"application/pdf","data_base64":"JVBERi0=","fallback":"pdf text"}]}}\n' "$id"
      ;;
    *plugin/shutdown*) break ;;
  esac
done
