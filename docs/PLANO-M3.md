# Telinhas — Plano do M3

> Decisões da seção 2 confirmadas pelo mantenedor (DEC-58). Escopo do M3 em `MARCOS.md`: captura real de monitor e janela no Windows, encode H.264 por hardware atrás da trait `Encoder` (DEC-39), fallback por software com aviso (RF-14), resolução e FPS vindos do hardware e da fonte (RF-05, RF-22), e a medição dos bitrates reais de 1080p60.

## 1. O que o M3 precisa entregar

| Entrega | Critério |
|---------|----------|
| Captura de monitor e de janela no Windows | Lista as fontes, captura a 60 fps estáveis em 1080p, com e sem cursor |
| Encode H.264 por hardware | Funciona em pelo menos a GPU da máquina de teste, com bitrate ajustável em tempo real (o GCC muda a estimativa a cada instante) |
| Fallback por software | `openh264` (já usado no spike), com um aviso visível de que pode ficar quebrado (DEC-15, RF-14) |
| Opções de resolução e FPS | Vêm da fonte e do hardware, não de uma lista fixa (DEC-36, RF-22) |
| Ligação com o `webrtc` | O spike passa a enviar a tela real em vez da fonte sintética |
| Medição | Bitrate real de 1080p60 para quatro tipos de conteúdo, mais uso de CPU/GPU e latência |

Fora do M3 (por já estarem em outros marcos): trocar a janela sem derrubar (M6), mouse como opção do usuário (M6), áudio (M5), interface (M4).

## 2. Decisões que dependem de você

### 2.1 Como acessar o encoder de hardware

| Opção | Prós | Contras |
|-------|------|---------|
| **A. FFmpeg** (`h264_nvenc`, `h264_qsv`, `h264_amf`) — já é a DEC-39 | Uma API para os três fabricantes; NVENC é acessado direto, sem depender do Media Foundation; bitrate muda em tempo real; vira a base também do macOS (VideoToolbox) e do Linux (VAAPI) | Build no Windows é mais trabalhoso: precisa das bibliotecas do FFmpeg (build LGPL compartilhada) e do LLVM (para o `bindgen`); as DLLs pesam no instalador (RNF-13 pede ≤ 100 MB) |
| **B. Media Foundation** (API do próprio Windows, crate `windows`) | Sem dependências externas, nada para distribuir | Depende do fabricante expor um encoder (MFT) de H.264. Um projeto que testei na pesquisa relatou **NVIDIA sem MFT de encode H.264 nos drivers atuais** (uma máquina, uma versão de driver, então vale como alerta e não como regra). Só funciona no Windows: macOS e Linux precisariam de outro caminho |
| **C. SDKs direto** (NVENC, AMF, oneVPL) | Controle máximo, menor latência | Três integrações diferentes, muito mais trabalho |

**Decisão: A (FFmpeg)**, que já era a DEC-39, **mais uma sonda de descoberta antes de codificar** (passo M3a abaixo) para confirmar na sua máquina quais encoders o FFmpeg consegue abrir. A trait `Encoder` deixa o Media Foundation entrar depois como segundo backend, se algum fabricante falhar. Rejeitei chamar o `ffmpeg.exe` como processo separado: não dá para mudar o bitrate em tempo real sem reiniciar, e isso é essencial para o GCC.

### 2.2 Como capturar

| Opção | Prós | Contras |
|-------|------|---------|
| **`windows-capture`** (crate sobre o Windows Graphics Capture) | Pronto: monitor e janela, opção de cursor, borda, intervalo mínimo entre quadros | Dependência de terceiros; preciso confirmar se entrega a textura da GPU (para o caminho sem cópia) e não só a memória da CPU |
| **API do Windows direto** (crate `windows`) | Controle total da textura D3D11 | Bem mais código, mais chance de erro sem eu poder rodar |

**Minha recomendação era `windows-capture`, mas o mantenedor escolheu a API do Windows direto (crate `windows`).** Isso dá controle total da textura D3D11, o que facilita o caminho sem cópia depois, ao custo de mais código. A versão inicial continua a mesma: que **copia o quadro para a memória da CPU** (BGRA → NV12 → encoder). É o caminho mais simples e já atende a medição do M3. O caminho sem cópia (textura da GPU direto para o encoder) fica como otimização, caso o uso de CPU passe da meta (RNF-10, ≤ 15% de um núcleo). Se o `windows-capture` não expuser a textura, troco por API direta só nessa etapa.

### 2.3 Que GPU tem a máquina Windows

**NVIDIA** (mantenedor). O `nvidia-smi` visto do WSL mostra uma **GeForce RTX 2070 SUPER**, driver 621.14. O encode de hardware é o NVENC, pelo `h264_nvenc` do FFmpeg, sem depender do Media Foundation. AMD e Intel ficam sem teste por enquanto. Se a máquina for um notebook com GPU integrada e dedicada, a sonda do M3a mostra qual o FFmpeg está usando.

## 2.4 Resultado da sonda (M3a, 15/11/2026)

Máquina de teste: Windows 11 build 10.0.26300, GeForce RTX 2070 SUPER (8 GB, driver 621.14), monitores 1920×1080 a 144 Hz (principal) e 2560×1080 a 60 Hz.

| Item | Resultado |
|------|-----------|
| Windows Graphics Capture | Disponível; a API de **desligar a borda** e a de **desligar o cursor** existem. Falta confirmar na prática que a borda realmente some sem pedir permissão (M3b) |
| `h264_nvenc` 1080p60 | Funciona: 322 fps (cerca de 3,1 ms por quadro) |
| `h264_nvenc` 1440p60 | Funciona: 286 fps (cerca de 3,5 ms por quadro) |
| `h264_mf` (Media Foundation) | Funciona: 313 fps em 1080p60 e 190 fps em 1440p60. Fica como plano B |
| `h264_qsv`, `h264_amf` | Falham, como esperado (não há GPU Intel nem AMD) |
| Lista de janelas | Funciona: mostra as janelas de Alt-Tab, com processo, título e tamanho |

Os tempos de encode incluem a geração do quadro sintético e a partida do `ffmpeg`, então o encoder real é no mínimo tão rápido quanto isso. Com cerca de 3 ms por quadro, o RNF-03 (≤ 8 ms) tem folga grande no NVENC.

## 2.5 Primeiras medições de captura (M3b, monitor de 144 Hz)

Quatro rodadas de 10 s, duas com a tela parada e depois com uma janela sendo arrastada, com e sem `--max-fps 60`.

| Achado | Leitura |
|--------|---------|
| Tela parada: 0 fps | O Windows Graphics Capture **só entrega quadro quando a imagem muda**. Para o app, isso exige repetir o último quadro (e emitir quadros-chave) quando nada muda, senão o espectador novo não recebe imagem e o encoder fica sem ritmo |
| Com movimento: intervalo p50 de 20,8 ms (48 fps) | É exatamente 3 atualizações de 144 Hz. Nem 144 nem 60 fps. A causa ainda não está clara: arrastar uma janela pode não gerar mais que isso, ou o nosso tratamento do quadro atrasava a devolução do buffer |
| Mesmo valor com e sem `--max-fps 60` | O teto de 60 não foi o fator limitante nesses testes, então não deu para ver o efeito dele |
| Cópia GPU→CPU: média de 2,5 a 4 ms, p95 de 7 a 8 ms | Alto para um orçamento de 16,7 ms por quadro a 60 fps. Confirma que o caminho sem cópia (textura direto para o encoder) vale a pena |
| CPU: 12 a 26% de um núcleo a 40 a 48 fps | Parte é espera pela GPU no `Map` |
| Janela do app difícil de arrastar no início (`--max-fps 60`) | Relato do mantenedor. Causa desconhecida, a investigar com testes A/B |

Mudanças feitas depois dessas medições: o pool passou de 2 para 4 buffers e o quadro é devolvido ao pool logo depois de enfileirar a cópia, em vez de depois da leitura.

### Segunda rodada de captura (movimento constante, TestUFO em tela cheia)

| Pedido ao Windows | Intervalo | fps |
|-------------------|-----------|-----|
| `--max-fps 144` | 6,9 ms | 142,6 |
| `--max-fps 72` | 13,9 ms | 71,7 |
| `--max-fps 60` | 20,8 ms | **48,0** |
| `--max-fps 30` | 34,7 ms | 27,7 |
| sem pedido | 20,8 ms | cerca de 48 |

**Conclusão:** o Windows Graphics Capture entrega só em múltiplos do vsync e **arredonda o intervalo mínimo para cima**: num monitor de 144 Hz, 60 fps viram 48 (3 atualizações), e sem pedido nenhum ele se comporta como um teto perto de 60. O app, então, pede sempre a taxa cheia do monitor e escolhe os quadros em software (`Pacer`, em `capture/src/pacer.rs`), o que dá 60 fps de verdade com intervalos de 13,9 ou 20,8 ms. Os testes unitários do `Pacer` cobrem 144→60, 144→30, 72→60, 60→60 e uma fonte mais lenta que o alvo. Quadros descartados pelo pacer não passam pela cópia da GPU.

**Verificado no Windows (pacer):** `--max-fps 60` no monitor de 144 Hz entregou **60,0 fps** (intervalos de 13,9 e 20,8 ms), `--max-fps 30` entregou **30,0 fps** (34,7 ms), e o ultrawide de 60 Hz também 60,0 fps. A CPU ficou em 13% de um núcleo a 60 fps e 7% a 30 fps, com a cópia da GPU em 1,5 a 2 ms por quadro. O monitor de 60 Hz também produz cerca de 144 quadros por segundo para o sistema, porque o compositor (DWM) roda na taxa do monitor mais rápido; o pacer escolhe os 60.

A medição varia com o que está na tela, porque o Windows só entrega quadro quando a imagem muda. Para medir fps é preciso conteúdo em movimento constante.

## 2.6 Bitrates medidos (M3e)

Método: `spike measure`, captura real do monitor 0, NVENC (`h264_nvenc`, preset p1, ajuste de baixa latência, sem quadros B, quadro-chave a cada 2 s), qualidade constante `cq:23` (o quanto o conteúdo pede). Os números de CPU do `measure` não valem (o pipe e as cópias não existem no app final). Conteúdo **sintético**: TestUFO em tela cheia.

| Conteúdo | Resolução e fps | Bitrate médio | p95 | Pico | Encoder (latência média) | GPU (geral) |
|----------|-----------------|---------------|-----|------|--------------------------|-------------|
| TestUFO "video game motion", stutter leve | 1080p60 | **18,9 Mbps** | 21,6 Mbps | 22,4 Mbps | **1,3 ms por quadro** | 49% |

| mesmo conteúdo, `cq:23` (repetição) | 1080p60 | 18,5 Mbps | 21,2 Mbps | 21,7 Mbps | 1,3 ms | 50% |
| mesmo conteúdo, `cq:28` | 1080p60 | **10,3 Mbps** | 12,1 Mbps | 12,5 Mbps | 1,2 ms | 48% |
| mesmo conteúdo, `cbr:8000` | 1080p60 | **8,01 Mbps** (e 8,04 na repetição) | 9,0 Mbps | 9,3 a 9,4 Mbps | n/d | 49% |
| mesmo conteúdo, `cq:23`, reduzido para 1280×720 | 720p30 | **6,0 Mbps** | 7,0 Mbps | 7,3 Mbps | **0,6 ms** | 32% |

Leituras:
- **O encoder respeita o alvo:** no modo CBR de 8 Mbps a média ficou em 8,0 Mbps, mas os picos por segundo chegaram a 9,3 e 9,4 Mbps (cerca de 17% acima). Para planejar o upload, usar o alvo com uma folga de cerca de 20%.
- **Qualidade constante:** subir o `cq` de 23 para 28 cortou o bitrate em cerca de 45% (18,9 para 10,3 Mbps).
- **Camada econômica:** 720p30 em `cq:23` precisou de cerca de um terço do bitrate do 1080p60 (6,0 contra 18,9 Mbps) para este conteúdo.
- **RNF-03 atendido com folga:** 1,3 ms por quadro, contra a meta de 8 ms.
- **Eu errei a previsão:** achei que o TestUFO daria um bitrate baixo, e deu 19 Mbps. Esse teste tem muito detalhe em movimento por toda a tela. Ele é um **limite superior** para conteúdo com movimento em tela cheia, não o caso típico de compartilhamento de tela (texto e janelas).
- **O que isso diz sobre a suposição de ~8 Mbps:** para esse tipo de conteúdo em qualidade alta, ela é pouco menos da metade do que o conteúdo pede. Com 11 espectadores, 19 Mbps seriam cerca de 209 Mbps de upload no transmissor, mais do que qualquer conexão residencial.
- **Consequência para o desenho (a decidir):** o app não deve deixar o bitrate seguir a qualidade do conteúdo. Deve ter um **bitrate alvo por predefinição** (resolução, fps e bitrate), com o encoder limitado a ele, e a estimativa de banda do RF-06 deve usar esse alvo, não o que o conteúdo pediria. Falta medir o conteúdo real (texto, rolagem, vídeo, jogo) e definir as predefinições.

### Tela parada: o CBR desperdiça banda (achado importante)

Com a área de trabalho **parada** (1 quadro novo em 30 s; o `measure` repete o último quadro a 60 por segundo), o modo `cbr:8000` continuou em **8,0 Mbps constantes**, segundo após segundo. O NVENC em CBR preenche o fluxo para manter a taxa exata, mesmo sem nada para codificar. Para compartilhamento de tela, isso queimaria 8 Mbps por espectador com a tela parada, justo no caso mais comum (alguém lendo um documento).

**Consequência:** o app não deve usar CBR. Deve usar qualidade constante com **teto de bitrate** (VBR com `-cq` e `-maxrate`), para que conteúdo fácil custe quase nada e conteúdo difícil fique limitado ao teto da predefinição. O comando `measure` ganhou o modo `capped:N:KBPS` para medir isso.

### Tela parada e modo com teto (medido)

| Situação | Modo | Média | Observação |
|----------|------|-------|------------|
| Tela parada, 1080p60 | `cq:23` | 0,87 Mbps | Alterna 41 kbps (quadros P) e 1,69 Mbps (o segundo do quadro-chave, a cada 2 s) |
| Tela parada, 1080p60 | `capped:23:8000` | 0,64 Mbps | Alterna 75 kbps e 1,2 Mbps pelo mesmo motivo |
| Conteúdo pesado, 1080p60 | `capped:23:8000` | **5,7 Mbps** | p95 de 8,0 Mbps, pico de 9,9 Mbps (cerca de 24% acima do teto); latência do encoder 2,1 ms |

Leituras:
- **O piso de uma tela parada é quase zero:** os quadros P custam cerca de 41 kbps. O que pesa é o **quadro-chave periódico** (cerca de 1,7 Mbit a cada 2 s). Com quadros-chave só **sob demanda** (espectador novo, pedido de PLI) e um periódico longo (por exemplo, a cada 10 s) como segurança, a média de uma tela parada cairia para algo como 0,2 Mbps.
- **O modo com teto funciona:** não gasta enchimento na tela parada (o CBR gastava 8 Mbps) e segura o conteúdo pesado perto do teto, com a média abaixo dele (5,7 contra 8 Mbps).
- **Planejamento do upload:** usar o teto com folga de 25% sobre o pico por segundo (os picos medidos foram de 17% a 24% acima do teto).

Qualidade a olho de 1080p60 a 8 Mbps e de 720p30 a 3 Mbps (CBR, conteúdo pesado): a avaliar pelo mantenedor.

## 2.7 Estado do M3c (encoder por bibliotecas)

**Feito e verificado (Linux):** crate `telinhas-codec` com a trait `Encoder` (DEC-39): `encode`, `request_keyframe`, `set_max_kbps` e `info`. Política de taxa da seção 2.6 (qualidade constante com teto, quadros-chave sob demanda). Backend de software (`openh264`) com 4 testes (primeiro quadro é chave, quadro-chave sob pedido, troca de teto, tamanhos ímpares recusados). Fallback automático para software com o motivo, para o aviso da DEC-15. Comando `spike encode`, que roda o encoder sobre a tela real ou uma imagem sintética, muda o teto no meio da execução e pede quadro-chave, e mede o tempo por quadro.

**NVENC verificado no WSL** (RTX 2070 SUPER, FFmpeg 9.0 LGPL compartilhado da BtbN, `ffmpeg-the-third` 6.0; o backend `codec/src/nvenc.rs` compilou na primeira e roda pela GPU por `libnvidia-encode`):

| Medida (1080p60, imagem sintética com ruído nível 4) | Resultado |
|------|-----------|
| Tempo de encode por quadro (inclui copiar o quadro para o FFmpeg) | p50 4,2 ms, p95 5,2 a 5,9 ms. **RNF-03 (≤ 8 ms) atendido** |
| Primeiro quadro | até 33 ms (abertura da sessão do NVENC) |
| Trocar o teto de 8 para 3 Mbps **reabrindo** o encoder | **449 ms** parado, cerca de 28 quadros perdidos |
| Trocar o teto **no lugar** (`set_max_bit_rate`) | **0 ms**, nenhum quadro perdido, um quadro-chave; a média desce de 8 para 3 Mbps em cerca de 6 s |
| Quadro-chave sob pedido | chega em até 1 s (aparece no relatório do segundo seguinte) |
| Respeito ao teto de 8 Mbps com este ruído | média de 9,0 Mbps (12% acima), pico de 11,6 Mbps (45% acima). Com o TestUFO real o pico foi 24% acima: a folga de planejamento depende do conteúdo |

O padrão do backend é a troca no lugar. O teto novo leva alguns segundos para se refletir no bitrate, então o controle de congestionamento deve mudar o teto com histerese (a DEC-44 já prevê reconfigurar o encoder), e não a cada relatório.

O restante do M3c, no Windows: confirmar o mesmo NVENC com a captura real.

**Para compilar no Windows (uma vez):**
1. Baixar a build **LGPL compartilhada** do FFmpeg 9.0 para Windows (BtbN, `ffmpeg-n9.0-latest-win64-lgpl-shared-9.0.zip`), extrair em `C:\ffmpeg` e definir `FFMPEG_DIR=C:\ffmpeg` e `C:\ffmpeg\bin` no `PATH`.
2. Instalar o LLVM: `winget install LLVM.LLVM` e definir `LIBCLANG_PATH` para a pasta `bin` dele.
3. Compilar: `cargo build --release -p telinhas-spike --features ffmpeg`.
4. Testar: `telinhas-spike.exe encode --source monitor:0 --fps 60 --seconds 30 --max-kbps 8000 --cap-change 10:3000 --keyframe-at 20 --backend hardware`.

O build LGPL é o que serve para distribuir o app (a build "full" do gyan.dev é GPL).

## 3. Passos

Cada passo termina com algo que você roda e me devolve.

**M3a. Sonda de descoberta.** Um comando `spike probe` que lista: monitores (tamanho, taxa de atualização), janelas capturáveis, GPUs, e quais encoders H.264 de hardware o FFmpeg consegue **abrir de verdade** em 1080p60 (abrir, não só constar na lista). Você me manda a saída e eu confirmo ou corrijo as decisões 2.1 e 2.2.

**M3b. Captura sem encode.** Captura monitor e janela, descarta os quadros e mede os fps entregues, a latência de captura e o uso de CPU, com e sem cursor. Isola problemas de captura dos de encode.

**M3c. Encode por hardware e por software.** A trait `Encoder` com os backends FFmpeg (hardware) e `openh264` (software, com aviso). Ajustes de baixa latência (sem quadros B, sem lookahead, GOP curto), mudança de bitrate em tempo real e pedido de quadro-chave (PLI/DEC-44). Mede o tempo de encode por quadro (RNF-03, ≤ 8 ms).

**M3d. Ligação com o `webrtc`.** `spike send --source monitor:N` ou `window:ID` em vez da fonte sintética, com o mesmo controle de fila do M2. Ensaio curto entre redes.

**M3e. Medição dos bitrates.** Bitrate médio e de pico, CPU e GPU, para 720p30, 1080p30, 1080p60 e 1440p60, em quatro conteúdos:
1. área de trabalho com texto e digitação;
2. rolagem de página web;
3. vídeo em reprodução;
4. jogo ou movimento rápido.

Medido em qualidade constante (o quanto o conteúdo "pede") e em bitrate fixo (como fica a imagem). Substitui a suposição de ~8 Mbps por espectador e fecha a fórmula do RF-06.

## 4. Organização do código

- `capture/`: crate `telinhas-capture`, com a trait `Capturer` (lista fontes, inicia a captura, entrega quadros). Implementação do Windows direto sobre a API (`windows`: Windows Graphics Capture e DXGI), atrás de `cfg(windows)`, de modo que o workspace continue compilando em Linux e macOS.
- `codec/`: crate `telinhas-codec`, com a trait `Encoder` (DEC-39) e os backends.
- `spike/` passa a depender dos dois.
- Um quadro entregue pela captura é um tipo que pode ser memória da CPU agora e textura da GPU depois, sem mudar a trait.

## 5. Como vamos trabalhar (e o risco)

Eu **não consigo rodar captura nem encode por hardware daqui**: o ambiente é Linux no WSL. O trabalho é em ciclos curtos, como no M2: eu escrevo, você compila e roda no Windows, e me devolve a saída.

Para reduzir idas e vindas:
- O código puro Rust (a captura) eu devo conseguir **verificar a compilação para Windows a partir do Linux** (`cargo check --target x86_64-pc-windows-msvc`), sem ligar. Vou confirmar isso logo no começo.
- A parte do FFmpeg depende das bibliotecas no Windows, então só compila aí. É a parte de maior risco do M3.
- Você vai precisar instalar, uma vez, o FFmpeg (build LGPL compartilhada) e o LLVM no Windows. Eu te passo os comandos quando chegarmos no M3c.

## 6. Pontos em aberto

- Qual crate Rust do FFmpeg usar (a decisão da captura já está tomada: API direta). O `ffmpeg-next` anuncia compatibilidade com o FFmpeg 4, e o `ffmpeg-the-third` acompanha versões mais novas. Preciso verificar qual funciona com o build de que você dispõe.
- Se o Windows Graphics Capture, na sua versão do Windows, desenha a borda amarela ao redor do que está sendo capturado. Isso importa para a DEC-18 (sem contorno). A API permite desligar a borda em versões novas do Windows 11; em outras pode não ser possível.
- Como a captura de janela se comporta com janelas minimizadas ou cobertas por outras.

## 7. Fontes

- Notas de benchmark de um projeto de encoder para Windows (disponibilidade do MFT de NVIDIA): <https://docs.rs/crate/mediaway-encoder/0.1.3/source/docs/windows/benchmarks.md>
- Documentação do crate de captura considerado e não adotado: <https://docs.rs/windows-capture>
- Media Foundation e vídeo com Direct3D 11 (Microsoft Learn): <https://learn.microsoft.com/uk-ua/windows/win32/medfound/supporting-direct3d-11-video-decoding-in-media-foundation>
