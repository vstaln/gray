#!/bin/sh
# Registry fixture: one valid provider and one malformed peer in manifest 1.2.
while IFS= read -r line; do
  case "$line" in
    *plugin/manifest*)
      id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9][0-9]*\).*/\1/')
      printf '%s\n' "{\"id\":$id,\"result\":{\"name\":\"provider-registry\",\"version\":\"0.1.0\",\"protocol\":\"1.2\",\"capabilities\":[\"provider.credentials\"],\"providers\":[{\"id\":\"provider-good\",\"name\":\"Good\",\"transport\":{\"kind\":\"openai-responses\",\"base_url\":\"https://example.test/v1\",\"authorization\":{\"kind\":\"bearer\",\"secret_name\":\"access_token\"},\"request\":{\"prompt_cache_key\":false,\"store\":false,\"include_reasoning_encrypted\":true,\"previous_response_id\":false,\"tool_choice\":\"auto\",\"parallel_tool_calls\":true,\"text_verbosity\":\"low\"},\"headers\":[{\"name\":\"originator\",\"value\":\"gray\"}]},\"auth_methods\":[{\"id\":\"example-login\",\"name\":\"Example login\",\"kind\":\"oauth\",\"operations\":[\"login\",\"refresh\",\"models\",\"revoke\"]}]},{\"id\":\"broken\",\"name\":\"Broken\",\"transport\":{\"kind\":\"openai-responses\",\"base_url\":\"http://example.test/v1\",\"authorization\":{\"kind\":\"bearer\",\"secret_name\":\"access_token\"},\"request\":{},\"headers\":[]},\"auth_methods\":[{\"id\":\"oauth\",\"name\":\"OAuth\",\"kind\":\"oauth\",\"operations\":[\"login\"]}]}]}}"
      ;;
    *plugin/shutdown*)
      exit 0
      ;;
  esac
done
