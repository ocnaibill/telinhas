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

## Ensaio 1 entre redes (Windows → MacBook, via Cloudflare Tunnel)

Transmissor no Windows, receptor no MacBook, sinalização em `wss://` atrás do Cloudflare Tunnel, sem TURN, 180 s, `--noise 4`.

**O que funcionou:** a sinalização pelo túnel, a conexão (`connected` sem relay), o vídeo e o data channel entre as duas redes, e a reação do GCC (a estimativa se moveu durante o teste).

**O que falhou:** o ensaio **não** atendeu o critério do M2. Dos cerca de 5400 quadros gerados, 1012 foram decodificados (cerca de 19%), e a latência de ida e volta cresceu sem parar, chegando a 48 s.

| Observação | Leitura |
|------------|---------|
| Receptor mostrou p50 ≈ 4,4 s já no primeiro quadro; o transmissor mediu p50 ≈ 0,44 s | Os relógios do Windows e do Mac diferiam em cerca de 4 s. A latência "pelo relógio de parede" do receptor é inútil, como previsto. Vale só o ida e volta do transmissor |
| Já nos primeiros 2 s só 16 quadros chegaram (cerca de 9 fps) | Igual ao loopback com `--noise 4`: o encoder pulava quadros porque o conteúdo pedia mais que o bitrate. Não era rede |
| A estimativa subiu até 2,7 Mbps, caiu a 150 kbps (piso) por volta de 45 s, voltou a 8 Mbps (teto) por volta de 80 s e caiu de novo | O GCC oscilou. Pode ser rede real ou efeito da fila do próprio transmissor (hipótese abaixo) |
| O ida e volta máximo cresceu cerca de 1 s por segundo entre 15 s e 60 s | Fila crescendo: o spike mantinha o encoder em pelo menos 300 kbps enquanto a estimativa estava em 150 a 450 kbps, e o `openh264` ainda estoura o bitrate. Sem limite de fila, o atraso não parava de crescer |

**Causa provável:** falha do spike, não da biblioteca. Faltavam (1) um piso de bitrate abaixo da estimativa e (2) descartar quadros quando há atraso acumulado. Além disso, `--noise 4` já sobrecarregava o encoder na partida, e eu indiquei esse valor no guia. Foi erro meu: ele é um teste de estresse, não um teste normal.

**Correção feita no spike:** o encoder segue a estimativa com piso de 100 kbps; quadros são descartados quando mais de 15 estão sem confirmação, com pedido de quadro-chave para retomar; os contadores agora separam quadros pulados pelo encoder, descartados pelo transmissor e perdidos na rede. A segunda rodada (`ENSAIO-M2.md`) começa com `--noise 3`.

**Ainda não sabemos** se a queda da estimativa foi congestionamento real do caminho (por exemplo, o upload do Windows ou o hotspot do Mac) ou consequência da fila. Só a segunda rodada responde.

## Ensaio 2 entre redes (mesma rota, spike corrigido, `--noise 3`, 180 s)

Windows (transmissor) → MacBook em 4G (receptor). Teste de velocidade do Mac no mesmo dia: fast.com 470 Mbps de descida e 55 de subida, latência 38 ms sem carga e **338 ms com carga** (fila grande no 4G); Ookla 595 Mbps, 28,7 Mbps de subida, ping 25 ms.

| Medida | Resultado |
|--------|-----------|
| Conexão e vídeo | Estáveis por 3 minutos, cerca de 30 fps nas janelas boas |
| Ida e volta até o quadro decodificado | p50 ≈ 96 a 109 ms o tempo todo (inclui encode e decode por software, cerca de 45 ms em loopback). p95 ≈ 120 ms nos primeiros 47 s |
| Picos | Perto de 48 s, 53 a 60 s, 70 a 84 s, 111 a 130 s e depois de 168 s: p95 subiu a 800 ms e o máximo a 2 s |
| Quadros | 5150 gerados, 4448 enviados, **701 descartados** pelo transmissor (13,6%), 1 pulado pelo encoder |
| Entregues | O receptor decodificou 4351 quadros até o fim da sua janela; as perdas que ele contou (498) batem com os descartes do transmissor. A perda real de rede foi próxima de zero (menos de 1%) |
| Controle de congestionamento | A estimativa ficou no teto de 8 Mbps o tempo todo, porque o fluxo usava só 0,3 Mbps. **O GCC não foi posto à prova** |
| Relógios | Diferença constante de cerca de 4,16 s entre Windows e Mac. Em cima desse valor, o receptor viu a latência de ida variar +25 ms (p95) até 47 s e depois até +290 ms, com pico de cerca de +950 ms perto de 58 s |

**Leitura.**
- O problema da fila infinita do ensaio 1 sumiu: a latência ficou limitada, e o p50 ficou perto de 100 ms. Isso atende a meta de p50 (RNF-01, ≤ 300 ms), ainda com encode e decode por software.
- Os picos de 0,3 a 1 s no vídeo são reais (o receptor também os viu) e combinam com o comportamento do 4G sob carga (latência de 338 ms no teste do fast.com). Provavelmente não é falha do `webrtc-rs`, mas a causa não está provada.
- Os 13,6% de quadros descartados são **culpa do meu critério de descarte**: ele contava quadros sem confirmação, e as confirmações viajavam por um data channel confiável e ordenado, que trava atrás de retransmissões. Na janela de 113 a 128 s as confirmações ficaram paradas enquanto o receptor continuava decodificando. Ou seja, descartei quadros que estavam chegando.
- Ao descartar 13,6%, esta rodada ficaria abaixo da meta de FPS (RNF-04, ≥ 95%), mas por causa do spike, não da rede.

**Correção feita:** as confirmações agora usam um data channel sem ordem e sem retransmissão, o limite de quadros em trânsito subiu de 15 para 30 (1 s) e as linhas de progresso mostram percentis **da janela de 2 s**, não só acumulados, para localizar cada pico.

**Falta:** repetir com `--noise 3` para confirmar a correção e rodar `--noise 4` para finalmente exercitar o GCC com banda de verdade.

## Ensaio 3 entre redes (`--noise 4`, 180 s, transmissor com build antigo)

Windows → MacBook, mesma rota. **Atenção:** o transmissor rodou o executável antigo (a saída não tem os contadores `generated=`), então esta rodada **não testa** a correção dos acks. O receptor (Mac) já tinha a versão nova, e as duas são compatíveis.

| Medida | Resultado |
|--------|-----------|
| Vazão recebida | cerca de 3,2 Mbps médios por 170 s, sem queda |
| Estimativa do GCC | subiu direto de 1,5 a 8 Mbps (o teto) em 22 s e ficou lá. Não houve o colapso do ensaio 1 |
| Ida e volta (transmissor) | p50 ≈ 213 ms estável; p95 de 514 caiu para 362 ms; máximo 610 ms. **Sem crescimento de fila** nos 3 minutos |
| Janelas de 2 s no receptor | p50 estável em torno de +137 ms sobre o ensaio 2; p95 entre 334 e 430 ms; só picos isolados no máximo de cada janela |
| Quadros por segundo | cerca de 7,4 fps (1253 quadros em 170 s). **O limite é o encoder por software**, não a rede: a 3,2 Mbps o conteúdo de `--noise 4` não cabe em 30 fps e o `openh264` pula quadros |

**Leitura.**
- O caminho aguentou 3,2 Mbps contínuos por 3 minutos sem fila crescente, e o GCC não precisou recuar. Isso é um bom sinal para a biblioteca.
- No ensaio 1, com o mesmo `--noise 4`, a estimativa colapsou para 150 kbps. Desta vez não. A diferença provável é o estado da rede móvel naquele momento, mas não está provada.
- `--noise 4` não serve como teste de fps: é conteúdo quase incompressível. O número de fps desta rodada diz pouco sobre o `webrtc-rs`.

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

Manter o `webrtc-rs` (DEC-38), **ainda sem concluir**. Entre redes diferentes ele conectou, entregou vídeo por 3 minutos com perda de rede próxima de zero e latência p50 de cerca de 100 ms. O que resta provar é o comportamento sob banda de verdade (o GCC ficou ocioso) e a estabilidade de 10 minutos. Os picos de latência parecem vir do 4G.
