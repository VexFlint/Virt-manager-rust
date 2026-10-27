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
watchdog, memballoon, panic, controller, input, sound, audio, video, shmem, memdev, iommu, pstore,
smartcard, tpm, rng, serial, parallel, channel, console, hostdev, redirdev.
`parsers/charsrc.rs` tem `add_char_source_args` e `add_device_seclabel_args`.
`boot.order` (`add_boot_order_arg`) aplica o `reorder_boot_order` do guest;
`disk` e `interface` já estão em `DOMAIN_DEVICES` só com os filhos comuns
(para o boot order) e ganham as propriedades quando forem portados.
O `--hostdev` lê os nodedevs de `parser::set_nodedevs` (o `virt-xml` busca
quando a opção é usada). No oráculo, `@FULL@` é o testdriver.xml.

## Falta (prioridade)
1. Parsers: disk, network, graphics, filesystem, vsock.
   Depois apagar o código legado de `lib.rs`.
2. Opções de guest (memory, vcpus, cpu, boot, features, clock, xml, metadata...)
   e o resto do virt-install (`--os-variant` etc.).
3. Só depois: GUI (ver CLAUDE.md seção 7).

## Bugs conhecidos
- `num_pcie_root_ports` e `--input none`/`--sound none` (skip_default_*) só
  validam; o virt-install vai precisar guardar esses valores quando migrar
  para o motor.

## Armadilhas já resolvidas (não reintroduza)
- O XML dos dispositivos tocados (`TouchedDev`) é lido só depois do
  `get_document_xml` do guest: aplicar o propstore duas vezes duplica
  filhos com índice além do fim (`codec1.type` sem `codec0`).
- O writer do `vendor/edit-xml` não escreve nós de texto vazios: assim
  `<devices>\n  </devices>` do libvirt sai indentado como no libxml2.
- `--edit -1`: o clap usa `allow_negative_numbers`.
