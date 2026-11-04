# Changelog

Mudanças de cada release do virt-manager-rust. O formato segue o
[Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/) e as versões
seguem o [SemVer](https://semver.org/lang/pt-BR/). A lista detalhada de PRs
de cada versão fica na página da release no GitHub.

A versão do pacote vem do `Cargo.toml` da raiz. O `--version` dos quatro
programas imprime a versão do upstream em que o porte se baseia (`5.1.0`),
igual ao virt-manager Python; a versão do pacote aparece no "Sobre".

## [0.9.0] - 2026-11-04

Baseado no virt-manager 5.1.0.

### Adicionado
- `virt-manager` (GUI GTK4): janela principal, detalhes da VM, consoles VNC,
  SPICE (com GL) e serial, Nova VM, detalhes do host, clonar, migrar,
  snapshots, preferências, bandeja e chaveiro.
- `virt-install`, `virt-xml` e `virt-clone` com as opções do upstream,
  conferidas byte a byte contra o upstream no harness da CI.
- Man pages e autocompletar de bash.
- Arquivos de instalação no layout `/usr` do upstream (`packaging/install.sh`).
- `virt-manager --version` imprime só a versão do upstream, como o argparse.
