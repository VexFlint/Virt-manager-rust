[English](CHANGELOG.md) · **Português (Brasil)**

# Changelog

Mudanças de cada release do virt-manager-rust. O formato segue o
[Keep a Changelog](https://keepachangelog.com/pt-BR/1.1.0/) e as versões
seguem o [SemVer](https://semver.org/lang/pt-BR/). A lista detalhada de PRs
de cada versão fica na página da release no GitHub.

A versão do pacote vem do `Cargo.toml` da raiz. O `--version` dos quatro
programas imprime a versão do upstream em que o porte se baseia (`5.1.0`),
igual ao virt-manager Python; a versão do pacote aparece no "Sobre".

## [Não lançado]

### Adicionado
- `bench/run.sh`: benchmark reproduzível contra o virt-manager Python
  (tempo e memória da CLI, abertura, memória e CPU ociosas da GUI), com os
  resultados em `bench/results/` e uma seção de Desempenho no README.

### Corrigido
- O `virt-install` carrega o banco do libosinfo uma vez por execução, e não a
  cada consulta: `--print-xml` de um convidado importado caiu de 3,4 s para 0,35 s.
- Detalhes, página da placa de rede: o ícone da placa (`network-idle`) vai
  embutido, então não aparece mais como imagem faltando em temas de ícones
  sem ele (Breeze, Adwaita 46+); "MAC address:", "active" e os avisos de rede
  são traduzidos, como no upstream.

## [1.0.0] - 2026-11-04

Baseado no virt-manager 5.1.0. Primeira versão estável.

### Adicionado
- Tradução: GUI e CLI usam gettext com os catálogos do próprio upstream
  (`po/`), então os textos batem com o virt-manager Python. Inclui português
  do Brasil (`pt_BR`); uma língua nova só precisa do arquivo `.po`.
- Seletor de idioma em Preferências > Geral ("Padrão do sistema" ou qualquer
  língua incluída). Vale ao reiniciar o virt-manager.
- Documentação em inglês e português (`*.pt-BR.md`).
- Os pacotes trazem os arquivos de tradução `.mo`.

### Corrigido
- Testes de clonagem instáveis na CI (geração de endereço MAC).
- O pacote rpm agora lista os arquivos de tradução.

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
