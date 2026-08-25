#!/bin/sh
printf '1.0.0 '
i=0
while [ "$i" -lt 12000 ]; do
  printf x
  i=$((i + 1))
done
