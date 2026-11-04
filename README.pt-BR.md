[English](README.md) · **Português (Brasil)**

> **Aviso:** este programa foi reconstruído com IA, principalmente como teste
> e prova de conceito do potencial de um modelo de fronteira (Claude, da
> Anthropic) e do seu harness para migrar um programa escrito em Python para
> Rust. Temos todo o respeito pelos desenvolvedores do
> [virt-manager original](https://github.com/virt-manager/virt-manager) e
> pelo esforço deles em desenvolver o projeto.

# virt-manager, em Rust

Uma reescrita do zero em Rust do [virt-manager](https://github.com/virt-manager/virt-manager)
(o app GTK/Python de gerenciar VMs pelo libvirt): todas as funções, o mesmo
layout da GUI, todos os drivers do libvirt (QEMU/KVM, Xen, LXC, bhyve), só
Linux.

**Status: porte completo do virt-manager v5.1.0, ainda pouco testado em uso
real.** Todas as janelas do upstream e as três ferramentas de linha de comando
foram portadas; veja [Status](#status) para as poucas diferenças e o que
ainda não foi exercitado em hosts reais.

## Por quê

A mesma GUI, mas toda em Rust: a ideia é algo mais leve e rápido que o
original em Python, sem mudar a aparência nem onde fica cada coisa.

## Status

| Peça | Estado |
|---|---|
| `virtinst-core` | Biblioteca. Todos os tipos de XML de dispositivo/domínio/rede/snapshot, a camada de conexão (bindings reais `virt`/libvirt), a lógica dos métodos de instalação, a lógica de clonagem (inclusive cópia de discos), o ajuste de permissões dos caminhos de disco. |
| `virt-install` | Binário funcionando. Todas as opções do upstream no motor de parsers 1:1, todos os métodos de instalação (`--location` com detecção de distro, `--cdrom`, `--pxe`, `--import`, `--install` kernel/initrd, `--cloud-init`, `--unattended`, `--reinstall`). |
| `virt-clone` | Binário funcionando. Todas as opções do upstream: `--auto-clone`, cópia de discos (`--file`, `--force-copy`, `--skip-copy`, `--nonsparse`, `--reflink`), `--preserve-data`, `--nvram`, `--mac`, `--replace`, `--print-xml`. |
| `virt-xml` | Binário funcionando. Todas as opções de dispositivo e de guest do upstream no motor de parsers, `--build-xml`/`--edit`/`--add-device`/`--remove-device`/`--update`/`--print-diff`, `--convert-to-q35`/`--convert-to-vnc`, `--osinfo`. Conferido byte a byte contra o `virt-xml` do próprio upstream (mais de 1300 casos no harness). |
| `virt-manager` (GUI) | GTK4, mesmo layout e textos do upstream. Janela principal (conexões, lista de VMs, colunas com gráficos), Adicionar Conexão com login por senha/chaveiro, Preferências, Sobre, ícone na bandeja (StatusNotifierItem). Nova VM (mídia local, URL, importar, manual, "Personalizar antes de instalar"), Apagar, Clonar, Migrar. Janela da VM: detalhes de todo tipo de hardware, Adicionar Hardware, a página de Desempenho, a aba XML, Snapshots, os consoles VNC/SPICE (precisam da libgvnc-1.0 e da libspice-client-glib-2.0) e os consoles seriais (precisam da vte-2.91-gtk4). Detalhes da Conexão: visão geral, redes virtuais (e Criar Rede), pools/volumes de armazenamento. Opções de inicialização `--show-*`. Usa o mesmo GSettings e as mesmas entradas do chaveiro do upstream, então conexões salvas e senhas de console continuam valendo. |

Diferenças conhecidas em relação ao upstream: sem inspeção pelo libguestfs (a
lista de Aplicativos fica escondida, como o upstream faz quando a inspeção não
está disponível); o ícone da bandeja precisa de um host StatusNotifierItem (o
GTK4 não tem `GtkStatusIcon`, então não há a alternativa via XEmbed).

Rode qualquer binário com `--help` para ver as opções exatas de agora: o texto
de ajuda diz, opção por opção, o que já está implementado e o que o upstream
faz diferente.

A interface do programa está em inglês, com as strings do próprio upstream.
Ainda não há traduções (os catálogos gettext do upstream não estão ligados).

## Compilar

```sh
cargo build --workspace
cargo test --workspace
```

Precisa do Rust (estável), de um ambiente de desenvolvimento do libvirt
(`libvirt-dev` ou equivalente) para linkar o crate `virt`, de
`libosinfo-1.0-dev` + `libclang-dev` para o `virtinst-core` e de
`libgtk-4-dev` (GTK 4.14+) para a GUI.

Teste a GUI com o driver de teste embutido do libvirt, sem precisar de daemon:

```sh
cargo run -p virt-manager -- --connect test:///default
```

## Instalar

Pacotes prontos (Ubuntu 24.04, Debian 13, Fedora, Arch e um tarball genérico)
estão na [página de Releases](https://github.com/VexFlint/Virt-manager-rust/releases).
Para compilar e instalar a partir do código:

O `packaging/install.sh` instala tudo no layout `/usr` do upstream
(binários, `.desktop`, metainfo AppStream, ícone hicolor, schema GSettings,
man pages, autocompletar do shell). Ele substitui uma instalação do
`virt-manager`/`virtinst` da distro, já que os caminhos e o schema são os
mesmos:

```sh
cargo build --release --workspace
sudo sh packaging/install.sh            # PREFIX=/usr por padrão
DESTDIR=/tmp/root sh packaging/install.sh  # área de staging, para pacotes
```

## Autocompletar do shell

`virt-install`, `virt-clone` e `virt-xml` completam as opções e as subopções
(`--disk source.<TAB>`) no bash, zsh e fish, pelo mesmo protocolo do
argcomplete do upstream. Os scripts ficam em `crates/virtinst-cli/data/`:

| Shell | Instalar em |
|-------|------------|
| bash | `bash-completion/*` → `/usr/share/bash-completion/completions/` |
| zsh | `zsh-completion/_*` → uma pasta do `$fpath` (também precisa dos arquivos do bash acima) |
| fish | `fish-completion/*.fish` → `~/.config/fish/completions/` |

## Man pages

`man/` tem as man pages do próprio upstream (`man/*.rst`, sem mudanças).
Gere com `man/build.sh` (precisa do `rst2man`, do `python3-docutils`); as
páginas vão para `target/man/` e são instaladas em `/usr/share/man/man1/`.

## Decisões de design

Toda decisão de arquitetura (toolkit da GUI, modelagem do XML, modelo
assíncrono, divisão em crates, licença, distribuição) foi discutida e
registrada antes de escrever código, não presumida. Veja
[`.scratch/rust-conversion/map.md`](.scratch/rust-conversion/map.md) e os
tickets ligados a ele em [`.scratch/rust-conversion/issues/`](.scratch/rust-conversion/issues/)
para o raciocínio completo de cada uma.

## Como contribuir

Veja o [`CONTRIBUTING.pt-BR.md`](CONTRIBUTING.pt-BR.md) (compilar, testar,
PRs), o [`AGENTS.md`](AGENTS.md) (regras do projeto, lidas também por agentes
de IA) e o [`docs/ROADMAP.md`](docs/ROADMAP.md) (o que falta e como reservar
um item).

## Licença

GPL-2.0-or-later, igual ao upstream. Veja [`LICENSE`](LICENSE).
