# Telinhas — Plano do M5 (áudio)

Rascunho para o mantenedor decidir. Cobre RF-08, DEC-16, DEC-28 e DEC-41.

## O que o áudio precisa fazer

1. **Quem transmite** captura o som e o envia junto do vídeo:
   - uma **janela** específica: só o áudio daquele aplicativo;
   - a **tela inteira**: o áudio do sistema, com a opção de **excluir aplicativos** (por exemplo, o Discord, para a voz de quem fala não voltar a ele);
   - o **microfone**, opcional e desligado por padrão.
2. **Quem assiste** ouve o áudio, com o volume por transmissão que a interface já desenha, e sem ficar fora de sincronia com a imagem.
3. Tudo cifrado como o vídeo (DTLS-SRTP, RNF-21) e sem custo de banda relevante perto do vídeo.

## O risco principal (DEC-41)

No Windows, o áudio por aplicativo vem da captura **por processo** do WASAPI (`AUDIOCLIENT_ACTIVATION_PARAMS`, Windows 10 versão 2004 ou mais nova). Ela tem dois modos:

| Modo | O que captura | Serve para |
|------|---------------|-----------|
| incluir uma árvore de processos | só o áudio daquele processo e dos filhos | transmitir uma janela (DEC-16) |
| excluir uma árvore de processos | o áudio do sistema inteiro, menos aquele processo e os filhos | tela inteira sem o Discord |

O modo de exclusão aceita **uma** árvore só. A DEC-16 pede excluir **vários** aplicativos. A saída provável é capturar, em vez de "tudo menos X", **uma captura de inclusão por aplicativo que não foi excluído** e misturar as capturas. Isso precisa ser provado em protótipo antes de virar garantia, como a DEC-41 já exige: o número de processos que fazem som muda o tempo todo, e processos novos aparecem durante a transmissão.

## Arquitetura proposta

```
captura (WASAPI) ─► mistura ─► Opus ─► track de áudio ─► WebRTC ─► Opus ─► jitter buffer ─► saída (cpal)
 sistema/aplicativo   (mic)    48 kHz    na mesma conexão            decode      ~60 ms          volume
```

- **Captura de áudio** no crate `telinhas-capture` (que já isola o código específico de cada sistema), atrás de uma interface comum, com um stub nos outros sistemas até o M7.
- **Uma faixa de áudio por transmissão, na mesma conexão WebRTC do vídeo.** A faixa já entra em toda oferta, então ligar o áudio depois não renegocia a conexão. O microfone é misturado no áudio da transmissão (DEC-16 o trata como fonte opcional dela).
- **Opus**, 48 kHz, estéreo, quadros de 20 ms, 96 a 128 kbps. O encoder do vídeo não muda.
- **Reprodução** no espectador com `cpal`, com um jitter buffer de uns 60 ms e ganho por transmissão (o controle de volume da interface). A sincronia com o vídeo começa simples: os dois chegam com atraso parecido (hoje 56 a 60 ms). Se o relógio do áudio derivar, ajustamos com os timestamps RTP.
- **Medição:** o painel F3 ganha a linha do áudio (atraso do buffer, quadros perdidos), no mesmo espírito do que já existe para o vídeo.

## Passos

1. **Spike de captura no Windows** (um binário em `spike/`, como no M3): lista as sessões de áudio, captura o sistema, um aplicativo, "tudo menos um" e "a mistura de vários incluídos", grava WAV e mede latência. Decide se a DEC-16 se cumpre como está escrita.
2. **Codec e transporte:** Opus entre dois pares em loopback, com teste automatizado.
3. **Reprodução e volume** no app, com a sincronia ao vídeo.
4. **Captura no app**: fonte de áudio no modal (já desenhado), exclusão de aplicativos, microfone.
5. **Fechamento:** medições, o RNF novo de atraso de áudio, e a prova de ponta a ponta.

## Decisões que preciso de você

1. **Biblioteca do Opus.**
   - **A. `opus` (libopus em C).** Qualidade e maturidade de referência. Exige compilar C com `cmake` em cada sistema (já temos o FFmpeg e o libclang no ambiente de build do Windows, então é mais uma dependência da mesma família).
   - **B. `opus-rs` (Rust puro).** Sem dependência de C, mas é recente e eu não o conheço em produção. O risco é qualidade ou bugs de borda.
   - Recomendo **A**. Se a instalação do `cmake` for um problema, trocamos por **B** e comparamos.
2. **Microfone misturado na faixa da transmissão (proposta) ou em uma faixa separada.** Misturar é mais simples e cabe na DEC-16. Uma faixa separada deixaria o espectador silenciar só a voz, mas é mais trabalho e muda a interface. Recomendo **misturar** por enquanto.
3. **Windows 10 anterior à 2004:** o áudio por aplicativo não existe lá. Recomendo exigir o Windows 10 2004 ou mais novo para o áudio, e dizer isso na interface (RNF-25, permissões e requisitos pedidos só quando o recurso é usado).
