#!/bin/sh
# spew_bytes.sh BYTES: emit exactly BYTES bytes (bounded-pump fixture).
# Usage: sh spew_bytes.sh 5242880  (= 5 MiB; log must be exactly BYTES long)
head -c "$1" /dev/zero | tr '\0' 'A'
