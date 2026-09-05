#!/bin/sh
# wide.sh: a single ~5 MB line including multibyte chars (codepoint-boundary test).
# 500000 x 10 bytes = 5,000,000 bytes, then multibyte tail, then newline.
awk 'BEGIN { for (i = 0; i < 500000; i++) printf "0123456789"; printf "h\xc3\xa9llo w\xc3\xb6rld\n" }'
