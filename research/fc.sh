#!/bin/zsh
# usage: fc.sh out url
for i in 1 2 3 4 5 6 7 8; do
  firecrawl scrape "$2" --only-main-content -o "$1" 2>err.txt && exit 0
  perl -e 'select(undef,undef,undef,8)'
done
cat err.txt; exit 1
