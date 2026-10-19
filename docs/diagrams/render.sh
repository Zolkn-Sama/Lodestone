#!/bin/sh
# Régénère les images PNG des diagrammes à partir de leurs sources Mermaid (.mmd).
# Prérequis : Node.js (npx télécharge mermaid-cli au premier lancement).
# Usage : sh docs/diagrams/render.sh            (tous les diagrammes)
#         sh docs/diagrams/render.sh 09-chat-rag (un seul)
set -eu
cd "$(dirname "$0")"
MMDC="npx -y @mermaid-js/mermaid-cli@11.4.2"

if [ "$#" -gt 0 ]; then
  files="$(for name in "$@"; do echo "${name%.mmd}.mmd"; done)"
else
  files="$(ls ./*.mmd)"
fi

for source in $files; do
  # -s 2 : image nette sur écran haute densité ; fond blanc lisible en thème sombre.
  $MMDC -i "$source" -o "${source%.mmd}.png" -s 2 -b white -q
  echo "✓ ${source%.mmd}.png"
done
