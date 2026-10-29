# CLAUDE.md

@AGENTS.md

As regras do projeto (1:1 com o upstream, fluxo de PR, build, harness, GUI,
roadmap) estão no `AGENTS.md`, importado acima, e valem para toda IA. Aqui só
o que é específico do Claude Code.

## Específico do Claude

1. **Responda sempre em português.**
2. **Fluxo de PR autônomo:** abra o PR, espere o `build-test` verde, faça o
   squash merge você mesmo, sem pedir confirmação. Só chame a pessoa se a CI
   falhar e você não conseguir corrigir, ou se houver conflito que não dê para
   resolver com segurança.
3. **O Vex (dono do repo) tem TDAH.** Ao falar com ele: mensagens curtas,
   listas curtas e priorizadas, a resposta ou o resultado na primeira linha.
   Nada de paredes de texto.
4. **Prints depois de cada merge que mexe em tela** (AGENTS.md, seção 4). Se a
   pasta `/mnt/project-files/screenshots/` existir (projeto do Vex no
   claude.ai), salve lá e linke pelo caminho; senão, anexe os prints no PR ou
   na resposta.
5. **Trabalho em paralelo:** outra pessoa também trabalha aqui com o próprio
   Claude. Antes de começar um item, siga a seção 2 do `docs/ROADMAP.md`
   (conferir PRs abertos e reservar com um PR draft).
