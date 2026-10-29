# Roadmap

O que falta para a cópia 1:1 do virt-manager v5.1.0 e o que vem depois.
Atualizado em 29/10/2026. Cerca de 90% do 1:1 está feito (PRs #53 a #72).

Este arquivo é a fonte da verdade do plano. Quem terminar um item marca ✅
com o número do PR **no mesmo PR** que entrega o item.

## 1. O que falta (em ordem de prioridade)

| # | Item | Escopo | Custo | Status |
|---|------|--------|-------|--------|
| 8 | GUI Snapshots | lista, criar, reverter, apagar (`virtManager/details/snapshots.py`, `ui/snapshots.ui`, `ui/snapshotsnew.ui`) | Caro | ✅ #75 |
| 16 | GUI Detalhes do host | visão geral + redes virtuais + criar rede (`host.py`, `hostnets.py`, `createnet.py`; a aba de storage já existe em `hoststorage.rs`) | Médio | ✅ #78 |
| 17 | GUI Clonar VM | `clone.py`, `ui/clone.ui` (inclui cópia de disco) | Médio | ✅ #77 |
| 18 | GUI Migrar VM | `migrate.py`, `ui/migrate.ui` | Médio | ✅ #76 |
| 22 | GUI Bandeja + login | ícone na bandeja e login com senha/chaveiro (`systray.py`, `lib/keyring.py`, `lib/connectauth.py`) | Médio | adiado |

Custo é uma estimativa grosseira de esforço (Caro > Médio > Barato).
Itens 8, 16, 17 e 18 são independentes entre si: dá para fazer em paralelo,
um por pessoa.

### Extras para o fim (só depois que tudo acima estiver pronto)

| # | Item | Escopo | Custo |
|---|------|--------|-------|
| 23 | Autocompletar bash/zsh/fish + man pages | `clap_complete` + `clap_mangen` | Barato, 1 PR |
| 24 | Console SPICE acelerado (DMA-BUF/GL) | `GdkDmabufTexture` (GTK 4.14), ganho com virgl; risco: drivers de GPU | Médio, 2 a 3 PRs |

### Fase 2 (só depois do 1:1 terminado)

- **Host Windows nativo, no modelo do Proxmox:** um segundo backend que
  controla o `qemu.exe` direto (aceleração WHPX, controle via QMP, sem libvirt).
- **Estimativa:** 15 a 25 PRs.
- **Primeiro passo:** um protótipo pequeno que liga uma VM com WHPX e a
  controla pelo QMP.

## 2. Como não duplicar trabalho

Mais de uma pessoa (e mais de uma IA) trabalha aqui. Antes de escrever
código para um item:

1. **Confira se está livre:** `git log --oneline`, os PRs abertos e as issues
   abertas. Se já existe PR ou issue para o item, escolha outro.
2. **Reserve:** abra um **PR draft** (ou uma issue) com o número do item no
   título, por exemplo `[#8] GUI Snapshots`, e mude o Status na tabela acima
   para `em andamento (@seu-usuario)` no primeiro commit.
3. **Item grande?** Divida em vários PRs pequenos (ex.: snapshots: lista
   primeiro, depois criar/reverter/apagar) e diga isso na reserva.
4. **Desistiu ou parou?** Feche o draft ou comente nele, e volte o Status
   para `livre`, para o item não ficar preso.
5. Se dois PRs tocarem os mesmos arquivos, quem mergear depois faz o rebase
   sobre o `master`.

## 3. Decisões já tomadas

- **Bandeja e login (#22):** `ksni` para o ícone na bandeja (StatusNotifier via
  D-Bus; o GTK4 não tem `StatusIcon`) e `oo7` para senha/chaveiro (Secret
  Service). Obrigatórios para manter o 1:1.
- **Sem libadwaita:** muda a aparência e quebra o 1:1.
- **Autocompletar/man pages (#23) e console SPICE GL (#24) só no fim**, depois
  que tudo estiver implementado, aplicados com cautela.
- **Fase 2 (host Windows) só depois do 1:1** terminado.

## 4. Já feito

CLI: todos os parsers do `virt-install` e do `virt-xml` no motor 1:1 (`--disk`,
`--network`, `--graphics`, `--cpu`, `--boot`, memória, resto do guest, parsers
pequenos), métodos de instalação (`--location`, `--cdrom`, `--pxe`, `--import`,
`--cloud-init`, `--unattended`), `virt-xml --convert-to-q35/vnc` e `--osinfo`.
Oráculo do harness com mais de 1300 casos do upstream.

GUI: janela principal, conexões, preferências, detalhes da VM (todas as
páginas de hardware, adicionar/remover dispositivo, aba XML), apagar VM, consoles VNC,
SPICE e serial, navegador de storage, criar pool e volume, Nova VM completa
(mídia local, URL, importar, manual, "Personalizar antes de instalar").

Detalhes: `git log --oneline`.
