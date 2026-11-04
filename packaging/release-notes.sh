#!/bin/sh
# Topo das notas da release (docs/RELEASES.md, seção 4). O "O que mudou"
# vem do --generate-notes do GitHub, por isso termina aqui.
#
# Uso: sh packaging/release-notes.sh <versão>
set -eu

v=$1
repo=${GITHUB_REPOSITORY:-VexFlint/Virt-manager-rust}

cat <<NOTES
## virt-manager-rust $v

Porte 1:1 do virt-manager 5.1.0 para Rust + GTK4. Substitui os pacotes
\`virt-manager\`/\`virtinst\` da distro (mesmos caminhos e mesmas configurações).

### Destaques
- (editar antes de publicar: 3 a 5 linhas, o que o usuário percebe)

### Instalar
| Sistema | Arquivo | Comando |
|---|---|---|
| Outras | \`virt-manager-rust-$v-x86_64-linux.tar.gz\` | ver \`INSTALL\` no tarball |
| Windows | em breve (Fase 2) | |

Verificar: \`sha256sum -c SHA256SUMS\` e
\`gh attestation verify arquivo --repo $repo\`.

NOTES
