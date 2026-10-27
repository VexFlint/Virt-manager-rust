# CLAUDE.md

Instruções para sessões do Claude Code neste repositório. Leia tudo antes de
começar; é curto de propósito.

## 1. Regras fixas (as mais importantes primeiro)

1. **Responda sempre em português.**
2. **Cópia carbono 1:1 do virt-manager Python.** A referência é o upstream
   [virt-manager v5.1.0](https://github.com/virt-manager/virt-manager/tree/v5.1.0):
   `ui/*.ui` (layout, rótulos, menus, colunas) e `virtManager/*.py`
   (comportamento). Cada tela porta o layout, os textos, os menus, as colunas e
   o comportamento do original. **Não invente UX e não corte funções.**
   Bibliotecas modernas são bem-vindas (ex.: GTK4 `ListView` no lugar de
   `TreeView`) desde que o resultado visível para o usuário seja o mesmo.
3. **Fluxo de PR autônomo** (seção 2). Não peça para o Vex clicar em merge.
4. **O Vex tem TDAH.** Mensagens curtas, listas curtas e priorizadas, a
   resposta ou o resultado na primeira linha. Nada de paredes de texto.
5. **Mostre o progresso com prints** depois de cada merge que mexe em tela
   (seção 4).

## 2. Fluxo de PR

1. Um PR pequeno por tela ou por pedaço de tela. Nunca junte várias telas
   num PR só.
2. Antes de abrir o PR, rode localmente o mesmo que a CI (seção 3).
3. Abra o PR, espere o workflow **`CI`** (job **`build-test`**) ficar verde e
   veja se não há comentário de revisão bloqueante.
4. Faça o merge você mesmo, **squash merge**, sem pedir confirmação.
5. Depois do merge, mande ao Vex uma mensagem curta em português dizendo o que
   entrou, com os prints.

**Só chame o Vex quando:**
- a CI falhar e você não conseguir corrigir, ou
- houver conflito que você não consiga resolver com segurança.

Nunca faça merge com CI vermelha, com conflito ou com revisão bloqueante
aberta. Nunca desative ou pule um teste para deixar a CI verde.

## 3. Build, testes e harness

Dependências de sistema (Ubuntu 24.04, as mesmas de `.github/workflows/ci.yml`):

```sh
sudo apt-get update
sudo apt-get install -y libvirt-dev libosinfo-1.0-dev libclang-dev pkg-config \
  libxml2-utils libgtk-4-dev libgvnc-1.0-dev libspice-client-glib-2.0-dev \
  libvte-2.91-gtk4-dev libgtksourceview-5-dev python3-libvirt python3-libxml2 python3-requests
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

Harness de comparação com o upstream (golden corpus, ticket 19):

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

Nomeie os prints como `<nº do PR>-<tela>.png`. Se a pasta
`/mnt/project-files/screenshots/` existir, salve lá e linke pelo caminho;
senão, anexe os prints na resposta ao Vex.

Dicas do ambiente:
- Sem D-Bus, o dconf não grava: use `VIRT_MANAGER_TEST_MEMORY_SETTINGS=1`.
- USB hotplug não funciona no container (não há `/dev/bus/usb`).
- Console VNC: `qemu-system-x86_64 -vnc 127.0.0.1:1`. SPICE:
  `qemu-system-x86_64 -vga qxl -display none -spice port=5904,addr=127.0.0.1,disable-ticketing=on`.
  O Xspice trava no container, não perca tempo com ele.

## 5. Decisões de design da GUI (`crates/virt-manager`)

- **GTK4** via `gtk4` 0.11 com a feature `v4_14` (o Ubuntu 24.04 tem GTK 4.14).
- **Mesmo schema GSettings do upstream** (copiado em `crates/virt-manager/data`),
  então as configurações são compartilhadas com o virt-manager Python. O
  `build.rs` compila uma cópia de reserva em `OUT_DIR`.
- **App id diferente** (`org.virt-manager.virt-manager-rust`) para não brigar
  pela instância única no D-Bus com um virt-manager Python aberto.
- Ícones do upstream embutidos como GResource.
- **Async:** `gio::spawn_blocking` + `glib::spawn_future_local` (ticket 09).
  Eventos de domínio em `connevents.rs`; o tick ainda faz polling de estatísticas.
- **Consoles:** FFI escrita à mão para a parte sem GTK das bibliotecas, com
  widget GTK4 próprio: VNC em `gvnc.rs` + `vncdisplay.rs` (libgvnc), SPICE em
  `spice.rs` + `spicedisplay.rs` (libspice-client-glib), widget comum em
  `display.rs`. Serial com `vte4`. Loop de eventos do libvirt em `libvirtglib.rs`.
- Os nomes dos arquivos `.rs` seguem os módulos do upstream
  (`createvm.rs` ↔ `virtManager/createvm.py`, etc.). Siga esse padrão.
- Lógica sem GUI (XML, defaults, storage, instalação) fica no `virtinst-core`,
  espelhando o `virtinst/` do upstream.
- Telas ainda não portadas aparecem nos menus, mas desabilitadas.

Decisões de arquitetura e o raciocínio por trás delas:
`.scratch/rust-conversion/map.md` e `.scratch/rust-conversion/issues/`.

## 6. Onde ver o progresso

Não confie numa lista de status congelada aqui. Veja o estado real em:
- `git log --oneline` (cada PR de tela tem um título descritivo);
- `crates/virt-manager/src/` (cada tela portada tem seu `.rs`);
- itens de menu ainda desabilitados na GUI;
- `--help` de cada binário da CLI (diz o que já foi implementado).

## 7. Roteiro do que falta (em ordem de prioridade)

GUI (uns 10 a 12 PRs):
1. Nova VM: instalação por URL e "Personalizar antes de instalar"
2. Snapshots
3. Detalhes do host (visão geral, redes virtuais, criar rede)
4. Clonar VM
5. Migrar VM
6. Aba XML (editor de XML nos detalhes)
7. Ícone na bandeja e login com senha/chaveiro

CLI (15 PRs ou mais): portar 1:1 as opções do `virt-install`, `virt-clone`
(cópia de disco) e `virt-xml`, que hoje só têm o básico.

Antes de começar um item, confira no `git log` se ele já não foi feito.
