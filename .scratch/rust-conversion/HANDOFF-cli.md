# Handoff: CLI (virt-install, virt-clone, virt-xml)

Estado para quem continuar o trabalho de CLI. Leia o CLAUDE.md antes.

## Como o motor funciona
- `crates/virtinst-core/src/xmlbuilder.rs`: port do `XMLBuilder` do upstream
  (`XmlClass` estático por dispositivo, `XmlObject`, propstore, ordem de props).
- `crates/virtinst-cli/src/parser.rs`: port do `VirtCLIParser` (`ParserDef`,
  `ArgDef`, `parse_new`, `parse_edit`, `find_objects_to_edit`, introspecção).
  Ganchos: `pre_parse` (mexe no optdict) e `custom_parse` (o `_parse`
  sobrescrito do upstream: devolve objetos próprios ou nada).
  `set_conn_support` guarda os `conn.support.*` que os parsers leem.
- `crates/virtinst-cli/src/parsers/`: um `ParserDef` por opção.
  `parsers::set_defaults` aplica o `dev.set_defaults(guest)` (com `guest_info`
  e domcaps para vídeo).
- `virt-xml.rs`: opções ainda não portadas passam pelo módulo `legacy`
  (código antigo de `lib.rs`). Ao portar uma, troque `legacy(...)` por
  `engine(...)` em `xml_options` (ordem = `VIRT_PARSERS` do upstream).

## Receita para portar um parser (1 grupo por PR)
1. Classe `XmlClass` no `devices/<x>.rs` do core (props, filhos, prop_order) +
   `set_defaults`; adicionar em `DOMAIN_DEVICES` (`guest.rs`).
2. `ParserDef` em `parsers/`, registrar em `parsers::all()`.
3. Trocar para `engine` no `virt-xml.rs`.
4. Casos no oráculo `.scratch/rust-conversion/harness/oracle-virt-xml.txt`
   (copie de `tests/test_cli.py` do upstream). Teste rápido: rode o harness.

## Portados no motor
watchdog, memballoon, panic, controller, input, sound, audio, video.

## Falta (prioridade)
1. Parsers: disk, network, graphics, serial/parallel/channel/console, hostdev,
   filesystem, smartcard, redirdev, tpm, rng, shmem, memdev, vsock, iommu,
   pstore. Depois apagar o código legado de `lib.rs`.
2. Opções de guest (memory, vcpus, cpu, boot, features, clock, xml, metadata...)
   e o resto do virt-install (`--os-variant` etc.).
3. Só depois: GUI (ver CLAUDE.md seção 7).

## Bugs conhecidos (achados pelo oráculo, ainda não corrigidos)
- `virt-xml VM --edit --sound codec1.type=duplex`: nós criamos `codec0` vazio
  e `codec1`; o upstream não (ver como `_make_find_inst_cb` cria só o índice
  pedido quando a lista é parseada do XML).
- `virt-xml test --add-device --sound ...` num guest com `<devices></devices>`
  vazio: o upstream re-renderiza `<devices>` com quebra de linha/indentação;
  nós inserimos inline.
- `--edit -1`: o clap recusa `-1` como valor (precisa `allow_hyphen_values`
  ou `allow_negative_numbers` no `--edit`) e `find_objects_to_edit` precisa
  aceitar índice negativo.
- `num_pcie_root_ports` e `--input none`/`--sound none` (skip_default_*) só
  validam; o virt-install vai precisar guardar esses valores quando migrar
  para o motor.
