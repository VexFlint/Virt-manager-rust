# Roadmap

O que falta para a cópia 1:1 do virt-manager v5.1.0 e o que vem depois.
Atualizado em 29/10/2026. O 1:1 está completo (PRs #53 a #85); falta só
o que está em "Extras" e a Fase 2.

Este arquivo é a fonte da verdade do plano. Quem terminar um item marca ✅
com o número do PR **no mesmo PR** que entrega o item.

## 1. O que falta (em ordem de prioridade)

| # | Item | Escopo | Custo | Status |
|---|------|--------|-------|--------|
| 8 | GUI Snapshots | lista, criar, reverter, apagar (`virtManager/details/snapshots.py`, `ui/snapshots.ui`, `ui/snapshotsnew.ui`) | Caro | ✅ #75 |
| 16 | GUI Detalhes do host | visão geral + redes virtuais + criar rede (`host.py`, `hostnets.py`, `createnet.py`; a aba de storage já existe em `hoststorage.rs`) | Médio | ✅ #78 |
| 17 | GUI Clonar VM | `clone.py`, `ui/clone.ui` (inclui cópia de disco) | Médio | ✅ #77 |
| 18 | GUI Migrar VM | `migrate.py`, `ui/migrate.ui` | Médio | ✅ #76 |
| 22 | GUI Bandeja + login | ícone na bandeja e login com senha/chaveiro (`systray.py`, `lib/keyring.py`, `lib/connectauth.py`) | Médio | ✅ #79 |
| 25 | GUI Desempenho | página Performance nos detalhes da VM (`details.py` `_refresh_stats_page`, `ui/details.ui`) | Barato | ✅ #80 |
| 26 | GUI opções `--show-*` | `--show-domain-creator/editor/performance/console/delete` e o repasse à instância aberta (`virtmanager.py`, `engine.py` `cli_command`) | Barato | ✅ #81 |
| 27 | GUI permissão nos caminhos | `check_path_search`/`fix_path_search` (`virtinst/devices/disk.py`, `device/addstorage.py`) | Médio | ✅ #82 |
| 28 | GUI aba XML no resto | aba XML em adicionar hardware, criar pool/volume e storage do host (`xmleditor.py`) | Médio | ✅ #83 |

Custo é uma estimativa grosseira de esforço (Caro > Médio > Barato).
Itens 8, 16, 17 e 18 são independentes entre si: dá para fazer em paralelo,
um por pessoa.

### Extras (próximo passo)

Custo: **P** = pequeno (1 sessão curta), **M** = médio, **G** = grande.
Itens sem dependência podem ser feitos em paralelo.

| # | PR | Escopo | Custo | Depende de | Status |
|---|----|--------|-------|------------|--------|
| 23a | Autocompletar bash/zsh/fish | o protocolo do argcomplete do upstream (`cli.autocomplete`) em `virt-install`, `virt-xml` e `virt-clone` (opções e sub-opções de `--disk` etc.); zsh via `bashcompinit` e fish com o script do argcomplete | P | nada | ✅ #88 |
| 23b | Man pages | as páginas do upstream (`man/*.rst`, via `rst2man`) para ficar 1:1; `clap_mangen` só se o `rst2man` não servir | P | nada | livre |
| 24a | SPICE GL: recebe o scanout | sinais `gl-scanout`/`gl-draw` do `SpiceDisplayChannel` na FFI (`spice.rs`), importa o DMA-BUF como `GdkDmabufTexture`; volta ao modo sem GL se falhar | M | nada | livre |
| 24b | SPICE GL: liga na GUI | usa o 24a no `spicedisplay.rs` quando a VM tem `<gl enable="yes"/>` (virgl), com fallback e prints | M | 24a | livre |
| 24c | SPICE GL: testes e cursor | cursor, redimensionar, escala e teste com virgl real; risco: drivers de GPU | P | 24b | livre |

### Fase 2: host Windows nativo (só depois dos Extras)

Objetivo: no Windows, um segundo backend que controla o `qemu.exe` direto
(aceleração WHPX, controle via QMP), sem libvirt, no modelo do Proxmox. A
GUI 1:1 continua igual; o que muda é quem responde por baixo.

Ordem: o protótipo (F2-1) decide se vale seguir. Se o WHPX não der conta,
paramos ali.

| # | PR | Escopo | Custo | Depende de |
|---|----|--------|-------|------------|
| F2-1 | Protótipo | binário de teste: liga uma VM com `qemu.exe -accel whpx`, conecta no QMP (`qmp_capabilities`, `query-status`, `system_powerdown`) | M | nada |
| F2-2 | CI no Windows | job `windows-latest` que compila o workspace (GTK4 via MSYS2/gvsbuild) sem libvirt | M | nada |
| F2-3 | Cliente QMP | crate `qemu-qmp`: socket/named pipe, comandos, eventos assíncronos, testes com QMP falso | M | F2-1 |
| F2-4 | Trait de backend | interface comum (listar, ligar, desligar, pausar, eventos, estatísticas); o código libvirt atual vira a 1ª implementação, sem mudar nada visível | G | nada |
| F2-5 | Libvirt opcional | feature `libvirt` no Cargo; build no Windows sem libvirt, telas que dependem dela ficam desabilitadas | M | F2-4, F2-2 |
| F2-6 | XML → linha do qemu | gerar argumentos do `qemu.exe` a partir do XML de domínio do `virtinst-core` (CPU, memória, disco, rede, vídeo) | G | F2-4 |
| F2-7 | Armazenamento das VMs | guardar o XML e o estado das VMs em `%APPDATA%` (o "libvirt" do backend: definir, apagar, listar) | M | F2-4 |
| F2-8 | Ciclo de vida | ligar, desligar, forçar, pausar, retomar e eventos via QMP, ligados na janela principal | M | F2-3, F2-6, F2-7 |
| F2-9 | Console | VNC/SPICE do `qemu.exe` em `127.0.0.1` nos widgets que já existem | M | F2-8 |
| F2-10 | Estatísticas | CPU, memória, disco e rede via QMP (`query-blockstats`, etc.) para a lista e a página Desempenho | M | F2-8 |
| F2-11 | Discos e storage | `qemu-img` para criar/redimensionar, pools como pastas | M | F2-7 |
| F2-12 | Rede | NAT do usuário (`-netdev user`), depois ponte/TAP (OpenVPN TAP ou similar) | G | F2-8 |
| F2-13 | Nova VM no Windows | o assistente de Nova VM usando o backend novo | M | F2-6, F2-11, F2-12 |
| F2-14 | Hotplug e editar | adicionar/remover dispositivo com a VM ligada (`device_add`/`device_del`) e editar com a VM desligada | G | F2-8 |
| F2-15 | Snapshots | snapshots internos do qcow2 via QMP (`snapshot-save`/`load`) | M | F2-8, F2-11 |
| F2-16 | Instalador | MSI ou ZIP com GTK, ícones e `qemu.exe` (ou apontar para o QEMU instalado) | M | F2-5 |

Total: 16 PRs (4 G e 12 M), dentro da estimativa de 15 a 25. Clonar e migrar
ficam para depois da Fase 2.

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
- **Autocompletar/man pages (#23) e console SPICE GL (#24):** agora são o
  próximo passo, aplicados com cautela (o GL sempre com fallback).
- **Fase 2 (host Windows) só depois do 1:1** terminado.

## 4. Já feito

CLI: todos os parsers do `virt-install` e do `virt-xml` no motor 1:1 (`--disk`,
`--network`, `--graphics`, `--cpu`, `--boot`, memória, resto do guest, parsers
pequenos), métodos de instalação (`--location`, `--cdrom`, `--pxe`, `--import`,
`--cloud-init`, `--unattended`), `virt-xml --convert-to-q35/vnc` e `--osinfo`.
Oráculo do harness com mais de 1300 casos do upstream.

GUI: janela principal, conexões (com login por senha e chaveiro),
preferências, ícone na bandeja, detalhes da VM (todas as páginas de hardware,
desempenho, adicionar/remover dispositivo, aba XML), snapshots, apagar,
clonar e migrar VM, consoles VNC, SPICE e serial, detalhes do host (visão
geral, redes virtuais, criar rede, storage), navegador de storage, criar pool
e volume (com aba XML), Nova VM completa (mídia local, URL, importar, manual,
"Personalizar antes de instalar"), correção de permissão nos caminhos de
disco e as opções `--show-*` do `virt-manager`.

Fica de fora, de propósito: a inspeção via libguestfs (`lib/inspection.py`).
A lista de aplicativos fica oculta, como o upstream faz sem suporte a
inspeção.

Detalhes: `git log --oneline`.
