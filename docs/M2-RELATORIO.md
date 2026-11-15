# Telinhas — Relatório do M2 (parcial)

> Parcial: falta o ensaio entre redes (`ENSAIO-M2.md`). A decisão final sobre o `webrtc-rs` (DEC-38) é do mantenedor.

## Situação do `webrtc-rs`

Desde a DEC-38 o projeto mudou: a linha 0.17 (acoplada ao Tokio) ficou só com correções, e a 0.20 em diante é uma camada assíncrona fina sobre um núcleo sem I/O, o crate `rtc`. A versão testada foi a **0.21.1** (`webrtc` e `rtc`). Segundo a documentação, ela traz controle de congestionamento GCC, pacer, FlexFEC e PLI periódico. Fontes: blog do projeto (`webrtc.rs/blog`).

## O que foi verificado (loopback, uma máquina Linux, WSL2)

Código em `spike/`. Dois peers, fonte sintética 1280×720 a 30 fps com contador de quadros e relógio desenhados na imagem, H.264 por software (`openh264`), sinalização pelo servidor real do M1.

| Item | Resultado |
|------|-----------|
| Negociação por `signal` (offer/answer, sem trickle) | Funciona, com o servidor de sinalização do M1 e dois processos separados |
| Trilha H.264 enviada e recebida | Funciona; todos os quadros decodificados, com contador e relógio corretos |
| Perda de quadros em loopback | 0 |
| Latência em loopback (encode e decode por software) | p50 ≈ 52 a 70 ms, p95 ≈ 70 a 90 ms |
| Data channel P2P (usado para medir o ida e volta sem depender de relógios sincronizados) | Funciona |
| Controle de congestionamento (GCC com TWCC) | Ativo: a estimativa sobe de 1,5 para cerca de 2,4 a 3 Mbps em 10 s de loopback |
| Interceptors padrão (NACK, relatórios) | Ativos |

## Achados que mudam o desenho

1. **A latência de loopback não é zero por causa do `SampleBuilder`.** Ele só entrega um quadro quando chega o primeiro pacote do próximo (cerca de 33 ms a 30 fps). Dá para eliminar usando o bit de marcador do RTP. Fica como otimização do M3.
2. **Excesso de demanda vira queda de quadros e latência.** Com conteúdo que exige mais que a estimativa (`--noise 4` e `5`), o encoder pula quadros: caiu para cerca de 8 fps e a latência subiu para 280 a 500 ms. O `openh264` não segura o bitrate sem pular quadros. Isso mostra que o controle de taxa do encoder importa tanto quanto o da rede; no M3, com encoder de hardware, precisa ser reavaliado.
3. **Simulcast talvez não seja necessário.** A biblioteca tem exemplo de simulcast (receber e repassar camadas), mas no nosso desenho cada espectador tem a **própria conexão** com o transmissor (malha, DEC-06). Então a camada "original" ou "econômica" (DEC-04, DEC-05) pode ser escolhida alimentando cada conexão com o encoder certo, sem simulcast. Isso evita depender dessa parte da biblioteca. Vale registrar como DEC quando o M6 começar.
4. **PLI (pedido de quadro-chave) não está tratado no transmissor.** O spike força um quadro-chave a cada 2 s. Falta o transmissor reagir ao PLI do espectador que entra no meio ou perde pacotes. A biblioteca expõe RTCP do lado do receptor; o lado do transmissor ainda precisa ser verificado.
5. **API:** é preciso depender dos dois crates (`webrtc` e `rtc`). A API é assíncrona com `async_trait`, e tudo roda no Tokio, o mesmo do servidor.

## O que falta

| Item | Como |
|------|------|
| Estabilidade de 10 minutos entre redes diferentes | Ensaio do mantenedor (`ENSAIO-M2.md`) |
| Conexão por TURN | Ensaio, se o P2P direto falhar |
| Reação a perda e queda de banda | Ensaio com rede limitada (ou `tc netem` com `sudo`, que não está disponível no ambiente do desenvolvimento) |
| Reação ao PLI | Implementar e testar |
| Simulcast | Só se o desenho mudar (item 3) |
| Janela de exibição | M4, com a UI (DEC-40, DEC-56) |

## Recomendação provisória

Manter o `webrtc-rs` (DEC-38). Até aqui ele cobre o que o M2 exige (trilha H.264, data channel, congestionamento e interceptors) sem precisar trocar de biblioteca. A recomendação só vira definitiva depois do ensaio entre redes.
