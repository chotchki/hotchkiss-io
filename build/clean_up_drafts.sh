#!/bin/bash
# With thanks from here: https://gist.github.com/LUN7/0276596588f88335325c56873cf401c1
gh release list | grep Draft |  awk '{print $1 " \t"}' |  while read -r line; do gh release delete -y "$line"; done
