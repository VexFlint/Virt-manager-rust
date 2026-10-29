# AGENTS.md

Regras para qualquer agente de IA (Claude Code, Codex, Copilot, Cursor,
Gemini...) e para humanos que trabalham neste repositório. É curto de
propósito: leia tudo antes de começar.

Mais contexto:
- [`CONTRIBUTING.md`](CONTRIBUTING.md): compilar, testar e abrir PR, passo a passo.
- [`docs/ROADMAP.md`](docs/ROADMAP.md): o que falta, em ordem, e como dividir
  o trabalho sem duplicar.

## 1. Regras fixas (as mais importantes primeiro)

1. **Responda e escreva em português** (mensagens, commits, títulos de PR).
2. **Cópia carbono 1:1 do virt-manager Python.** A referência é o upstream
   [virt-manager v5.1.0](https://github.com/virt-manager/virt-manager/tree/v5.1.0):
   `ui/*.ui` (layout, rótulos, menus, colunas) e `virtManager/*.py`
   (comportamento); `virtinst/` e `virt-install`/`virt-xml`/`virt-clone` para
   a CLI. Cada tela porta o layout, os textos, os menus, as colunas e o
   comportamento do original. **Não invente UX e não corte funções.**
   Bibliotecas modernas são bem-vindas (ex.: GTK4 `ListView` no lugar de
   `TreeView`) desde que o resultado visível para o usuário seja o mesmo.
3. **Um PR pequeno por tela ou por pedaço de tela** (ou por grupo de opções da
   CLI). Nunca junte várias telas num PR só.
4. **Antes de começar um item do roadmap, reserve-o** (seção 2 do
   [`docs/ROADMAP.md`](docs/ROADMAP.md)) para não duplicar trabalho.
5. Nunca desative, pule ou apague um teste para deixar a CI verde.

## 2. Fluxo de PR

O `master` tem ruleset: **PR obrigatório**, check **`build-test`**
obrigatório, sem force push e sem apagar a branch.

1. Crie uma branch nova a partir do `master` atualizado para cada PR
   (`git fetch origin master && git checkout -b <branch> origin/master`).
   Nunca reaproveite uma branch cujo PR já foi mergeado.
2. Antes de abrir o PR, rode localmente o mesmo que a CI (seção 3).
3. Abra o PR (pode começar como draft para reservar o item), espere o
   workflow **`CI`** (job **`build-test`**) ficar verde e confira se não há
   comentário de revisão bloqueante.
4. Faça **squash merge**. Título do PR = título do commit: descritivo, em
   português (ex.: `GUI: Snapshots (lista, criar, reverter, apagar)`).
5. No mesmo PR, marque o item como feito no `docs/ROADMAP.md`.

Nunca faça merge com CI vermelha, com conflito ou com revisão bloqueante
aberta. Se a CI falhar e você não souber corrigir, ou houver conflito que não
dê para resolver com segurança, pare e chame um humano.

## 3. Build, testes e harness

Dependências de sistema (Ubuntu 24.04, as mesmas de `.github/workflows/ci.yml`):

```sh
sudo apt-get update
sudo apt-get install -y libvirt-dev libosinfo-1.0-dev libclang-dev pkg-config \
  libxml2-utils libgtk-4-dev libgvnc-1.0-dev libspice-client-glib-2.0-dev \
  libvte-2.91-gtk4-dev libgtksourceview-5-dev python3-libvirt python3-libxml2 python3-requests \
  cpio xorriso genisoimage python3-docutils
```

Se adicionar uma dependência de sistema nova, adicione também no `ci.yml` e
explique no comentário dele.

Mesmas etapas da CI, nesta ordem:

```sh
cargo fmt --all --check        # use --all na raiz: sem ele o virtinst-core fica de fora
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace --no-fail-fast
```

Harness de comparação com o upstream (golden corpus, ticket 19; a CI também roda):

```sh
git clone --depth 1 --branch v5.1.0 https://github.com/virt-manager/virt-manager /tmp/virt-manager-upstream
UPSTREAM_REF=/tmp/virt-manager-upstream bash .scratch/rust-conversion/harness/run.sh
```

Oráculo: o harness roda o `virt-xml` do próprio upstream (a partir do clone)
com os mesmos argumentos do nosso e compara stdout, stderr e código de saída
byte a byte. Os casos ficam em `.scratch/rust-conversion/harness/oracle-virt-xml.txt`;
todo parser novo deve ganhar casos lá. Precisa de `python3-libvirt`,
`python3-libxml2` e `python3-requests` (se o `python3` padrão não as enxergar,
o harness tenta `python3.12`). O `virt-install` do upstream também roda
(`python3 ./virt-install ... --print-xml`).

Para trabalho de CLI, leia também `.scratch/rust-conversion/HANDOFF-cli.md`
(como o motor de parsers funciona e a receita para portar uma opção).

## 4. Testar a GUI e tirar prints

Sem daemon, com o driver de teste do libvirt:

```sh
cargo run -p virt-manager -- --connect test:///default
```

Com libvirt real no container (para VMs de verdade):

```sh
sudo apt-get install -y libvirt-daemon libvirt-daemon-driver-qemu libvirt-clients qemu-system-x86 dnsmasq
useradd -r libvirt-qemu; groupadd -r kvm; mkdir -p /var/log/libvirt
libvirtd -d; virtlogd -d
```

Prints (Xvfb no display `:99` + ImageMagick `import`):

```sh
Xvfb :99 -screen 0 1280x800x24 &
DISPLAY=:99 VIRT_MANAGER_TEST_MEMORY_SETTINGS=1 cargo run -p virt-manager -- --connect qemu:///system &
DISPLAY=:99 xdotool mousemove X Y click 1   # navegar
DISPLAY=:99 import -window root shot.png
```

Todo PR que mexe em tela deve ter prints na descrição ou num comentário,
nomeados `<nº do PR>-<tela>.png`, comparáveis com a mesma tela do upstream.

Dicas do ambiente:
- Sem D-Bus, o dconf não grava: use `VIRT_MANAGER_TEST_MEMORY_SETTINGS=1`.
- USB hotplug não funciona em container (não há `/dev/bus/usb`).
- Console VNC: `qemu-system-x86_64 -vnc 127.0.0.1:1`. SPICE:
  `qemu-system-x86_64 -vga qxl -display none -spice port=5904,addr=127.0.0.1,disable-ticketing=on`.
  O Xspice trava em container, não perca tempo com ele.

## 5. Decisões de design da GUI (`crates/virt-manager`)

- **GTK4** via `gtk4` 0.11 com a feature `v4_14` (o Ubuntu 24.04 tem GTK 4.14).
- **Sem libadwaita**: ela muda a aparência e quebra o 1:1.
- **Mesmo schema GSettings do upstream** (copiado em `crates/virt-manager/data`),
  então as configurações são compartilhadas com o virt-manager Python. O
  `build.rs` compila uma cópia de reserva em `OUT_DIR`.
- **App id diferente** (`org.virt-manager.virt-manager-rust`) para não brigar
  pela instância única no D-Bus com um virt-manager Python aberto.
- Ícones do upstream embutidos como GResource, registrada antes de criar a
  `gtk::Application` (se o tema de ícones carregar antes, os ícones saem
  como image-missing).
- **Async:** `gio::spawn_blocking` + `glib::spawn_future_local` (ticket 09).
  Eventos de domínio em `connevents.rs`; o tick ainda faz polling de estatísticas.
- **Consoles:** FFI escrita à mão para a parte sem GTK das bibliotecas, com
  widget GTK4 próprio: VNC em `gvnc.rs` + `vncdisplay.rs` (libgvnc), SPICE em
  `spice.rs` + `spicedisplay.rs` (libspice-client-glib), widget comum em
  `display.rs`. Serial com `vte4`. Loop de eventos do libvirt em `libvirtglib.rs`.
- Os nomes dos arquivos `.rs` seguem os módulos do upstream
  (`createvm.rs` ↔ `virtManager/createvm.py`, etc.). Siga esse padrão.
- Lógica sem GUI (XML, defaults, storage, instalação) fica no `virtinst-core`,
  espelhando o `virtinst/` do upstream. Parsers da CLI ficam no `virtinst-cli`.
- Telas ainda não portadas aparecem nos menus, mas desabilitadas.

Decisões de arquitetura e o raciocínio por trás delas:
`.scratch/rust-conversion/map.md` e `.scratch/rust-conversion/issues/`.

## 6. Onde ver o progresso

- [`docs/ROADMAP.md`](docs/ROADMAP.md): o que falta e quem está fazendo.
- `git log --oneline` (cada PR tem um título descritivo);
- `crates/virt-manager/src/` (cada tela portada tem seu `.rs`);
- itens de menu ainda desabilitados na GUI;
- `--help` de cada binário da CLI (diz o que já foi implementado).

Antes de começar um item, confira no `git log` e nos PRs abertos se ele já
não foi feito ou está em andamento.
