#!/bin/bash
gh release list | grep Draft |  awk '{print $1 " \t"}' |  while read -r line; do gh release delete -y "$line"; done
