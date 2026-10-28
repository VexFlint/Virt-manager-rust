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
watchdog, memballoon, panic, controller, input, sound, audio, video, network, graphics, filesystem, vsock,
shmem, memdev, iommu, pstore,
memory, memtune, blkiotune, memorybacking, numatune, iothreads, metadata, events,
resource, features, clock, pm, sysinfo, qemu-commandline, xml (opções de guest),
smartcard, tpm, rng, serial, parallel, channel, console, hostdev, redirdev.
`parsers/charsrc.rs` tem `add_char_source_args` e `add_device_seclabel_args`.
O propstore segue o `_nonxml_fset` do upstream: setar de novo move a chave para o fim.
Opções de guest: sem `guest_propname` o objeto é o próprio guest (`parse_guest_edit`);
um `guest_propname` que é propriedade int (`--iothreads`) imita o upstream, que falha
no edit (`parse_scalar_edit`). `--build-xml --iothreads` precisa do `Guest.set_defaults`
completo (vem com o virt-install) e por enquanto dá erro.
`--xml` (`is_xml_action`) cria um `XMLManualAction` novo por opção e anexa ao guest;
os erros de validação dele saem sem o prefixo `Error: --x` (no upstream estouram no `get_xml`).
`--sysinfo` junta opções num sysinfo existente do mesmo tipo (`existing_inst`).
Elementos com prefixo (`qemu:`, `libosinfo:`) ganham o `xmlns` como no `_node_new`.
`ParseCtx.post_edit` roda efeitos no guest depois de um edit (ex.: `change_graphics`).
`boot.order` (`add_boot_order_arg`) aplica o `reorder_boot_order` do guest;
`--disk` está no motor (`parsers/disk.rs`, classe em `virtinst-core/src/devices/disk.rs`);
o `lib.rs` antigo e o `legacy` do virt-xml sumiram. O `_storage_backend` vive num
thread-local do parser (`disk::backend()`): `None` é o stub do upstream, e o
`get_source_path` resolve a `<source>` quando o stub não tem caminho.
Criação de storage: `StorageBackend::for_vol_install` (o `ManagedStorageCreator`);
o virt-xml cria o volume no define/`--start`/`--update` (`setup_device`) com o
`VolInstall` guardado no `TouchedDev`.
Erros `fail()` de dentro de callbacks usam `parser::fail` (sem o prefixo `Error: --x`).
`guest_info` não sincroniza o guest (senão o XML do dispositivo novo entra antes dos
defaults e a ordem dos atributos muda).
O `--hostdev` lê os nodedevs de `parser::set_nodedevs` (o `virt-xml` busca
quando a opção é usada). No oráculo, `@FULL@` é o testdriver.xml.
`--cpu`/`--vcpus`/`--cputune` em `parsers/cpu.rs`. O `--cpu` lê domcaps e o modelo
da CPU do host de `cpu::set_cpu_conn` (o virt-xml busca quando a opção é usada);
as security features só são calculadas quando um modo precisa delas. As
`force=a,force=b` viram uma lista separada por `\u{1f}` no `pre_parse`.
`--boot`/`--idmap`/`--seclabel`/`--keywrap`/`--launchSecurity` em `parsers/boot.rs`.
`ParserDef.inst_pre_parse` é o começo de um `_parse` que precisa do objeto
(o `_convert_boot_order`). `uefi` liga/desliga como `enable_uefi`/`disable_uefi`
lendo domcaps direto do `parser::connect()`. `XmlObject.clear` agora remove os
filhos de lista (como o upstream), não só limpa.
`Guest.set_defaults` está em `guest.rs` (`GuestOpts` guarda os
`skip_default_*`, `uefi_requested` e `num_pcie_root_ports`); o virt-xml usa
em `--build-xml --iothreads`. MagicURI (`__virtinst_test__...`) em
`cli::get_connection` vira as variáveis `VIRTINST_TEST_*`; no oráculo,
`@MAGIC@` e `@CAPS@` montam as URIs de teste do upstream.

O virt-install é o port do `virtinstall.py` (`bin/virt-install.rs`) sobre o motor,
com o `Installer` em `installer.rs`. Os parsers marcam os `skip_default_*`,
`uefi_requested`, `num_pcie_root_ports` e `cpu_special_mode_was_set` no
`parser::GuestFlags`, e o `GuestOpts::from_flags` lê de lá. `run_parser` escolhe
entre guest, escalar e lista. `cli::argparse_parse` imita o argparse (prefixos de
opção como `--dry`, erros `usage:`/`error:` com código 2). No virt-install os
erros do `--xml` saem no `get_xml` (`parser::defer_xml_action_checks` e
`XmlObject::check_xml_actions`). Oráculo: `oracle-virt-install.txt` (251 casos).

Instalação (#6): `installertreemedia.rs` (CLI) e `virtinst-core/src/install/`
(`urlfetcher` com HTTP via `ureq` e FTP próprio, `urldetect`, `installerinject`
com cpio/xorriso, `volumeupload`, `cloudinit`, `unattended`). Os scripts do
`--unattended` vêm do libosinfo (`osinfo/installscript.rs`). No testsuite o
upload usa o `_MockStream` e os caminhos viram `/VIRTINST-TESTSUITE/...`, como
no upstream.

virt-xml (#15): `--osinfo`, `--convert-to-q35`/`--convert-to-vnc`
(`parsers/convert.rs` guarda o pedido; `guest::convert_to_q35`/`convert_to_vnc`
rodam com a conexão) e o `--help` no formato do argparse
(`cli::argparse_help`, gerado do clap: use `verbatim_doc_comment` senão o clap
corta o ponto final; metavar = `value_name` ou o nome do campo em maiúsculas).
O `--update` reaproveita os dispositivos no define (`input_devs`), como o
upstream. Oráculo do test_cli.py: `oracle-virt-xml-upstream.txt` (160 casos;
`STDIN=`/`INPUT=` põem arquivo ou texto na entrada).

## Falta (prioridade)
1. GUI (ver CLAUDE.md seção 7). Casos de instalação que precisam de rede
   (http/ftp, `--install OSNAME`, drivers do Windows) ficam fora do oráculo.

## Bugs conhecidos
- Nenhum conhecido no corpus do oráculo.

## Armadilhas já resolvidas (não reintroduza)
- O XML dos dispositivos tocados (`TouchedDev`) é lido só depois do
  `get_document_xml` do guest: aplicar o propstore duas vezes duplica
  filhos com índice além do fim (`codec1.type` sem `codec0`).
- O writer do `vendor/edit-xml` não escreve nós de texto vazios: assim
  `<devices>\n  </devices>` do libvirt sai indentado como no libxml2.
- `--edit -1`: o clap usa `allow_negative_numbers`.
- O writer do `edit-xml` escapa atributos e texto como o libxml2
  (`libxml2_escape`); empurrar `(&str, &str)` no quick-xml escapava de novo.
- `VolInstall.capacity` é `u128`: `size=100000000000` estoura `u64`.
