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

**NVIDIA** (mantenedor). O encode de hardware é o NVENC, pelo `h264_nvenc` do FFmpeg, sem depender do Media Foundation. AMD e Intel ficam sem teste por enquanto. Se a máquina for um notebook com GPU integrada e dedicada, a sonda do M3a mostra qual o FFmpeg está usando.

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
