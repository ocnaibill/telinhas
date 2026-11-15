# Telinhas — Ensaio do M2 entre redes

> Guia para o mantenedor rodar o teste de 10 minutos do M2 (ver `MARCOS.md` e DEC-56). O código está em `spike/` e é descartável: serve só para responder se o `webrtc-rs` atende (DEC-38).

## O que o ensaio mede

Duas máquinas em **redes diferentes** (por exemplo, casa e celular em 4G ou a rede de outra pessoa) trocam um vídeo H.264 sintético de 1280×720 a 30 fps. Um lado transmite (`send`), o outro recebe (`recv`). Durante 10 minutos o programa imprime a cada 2 s:

- **No transmissor:** estimativa de banda do controle de congestionamento (GCC) e a latência de ida e volta até o quadro ser decodificado (`round trip`). Ela é medida só com o relógio do transmissor, então vale mesmo com relógios diferentes. Ela inclui o caminho de volta do aviso, então fica um pouco acima da latência de ida (vidro a vidro).
- **No receptor:** quadros decodificados, quadros perdidos (lacunas na contagem), taxa recebida e latência pelo relógio de parede. Esta última só vale se os dois relógios estiverem sincronizados (no WSL, em geral não estão). Use o `round trip` do transmissor como número principal.

## Metas a comparar (`REQUISITOS-NAO-FUNCIONAIS.md`)

| Meta | Valor |
|------|-------|
| RNF-01 latência vidro a vidro, P2P direto | ≤ 300 ms |
| RNF-02 o mesmo, via TURN | ≤ 500 ms |
| RNF-04 FPS entregue | ≥ 95% do configurado |
| RNF-15 perda tolerada sem congelar | até 2% |
| M2 (saída) | 10 minutos estáveis |

Lembre que o encode e o decode do ensaio são **por software** (`openh264`), mais lentos que o hardware do M3. Se a latência passar da meta, parte dela é do software.

## Preparação

1. **Rust** nas duas máquinas (`rustup`). No Windows, instale também as "Build Tools do Visual Studio" com C++ (o `openh264` compila código C++).
2. Clone o repositório e compile:

```bash
cargo build --release -p telinhas-spike -p telinhas-signaling
```

3. **Servidor de sinalização**, alcançável pelas duas máquinas. No homelab (DEC-21):

```bash
TELINHAS_BIND=127.0.0.1:8080 ./target/release/telinhas-signaling
```

Aponte o Cloudflare Tunnel para `http://localhost:8080`. As máquinas usam `wss://SEU-HOST/ws`. Para um teste rápido, o servidor também pode rodar na máquina do transmissor e ser exposto por um túnel temporário; a escolha de como expor é sua. Confirme que `https://SEU-HOST/healthz` responde `ok`.

## Rodando

**Máquina A (transmissor):** cria a sala e imprime o código.

```bash
./target/release/telinhas-spike send --server wss://SEU-HOST/ws --noise 4 --max-kbps 4000 --seconds 660
```

**Máquina B (receptor):** entra com o código impresso.

```bash
./target/release/telinhas-spike recv --server wss://SEU-HOST/ws --code XXXX-XXXX --seconds 600
```

Pontos de atenção:

- Inicie o receptor **depois** do transmissor imprimir `waiting for the receiver to join...`.
- `--noise` vai de 0 a 5 e controla o quanto o vídeo é difícil de comprimir. Em `4` o encoder quer mais de 1,5 Mbps (na máquina de desenvolvimento, em loopback). Com `0` a `3` o fluxo fica em torno de 300 kbps e não testa a banda.
- Se a conexão não chegar a `connected` em 30 s, a rede provavelmente bloqueia o UDP direto. Repita com TURN, se houver um (`coturn`):

```bash
... --turn-url turn:SEU-HOST:3478 --turn-user USUARIO --turn-pass SENHA
```

## O que me enviar

Copie as linhas que começam com `FINAL` dos dois lados, mais:

- o tipo de rede de cada ponta (Wi-Fi de casa, 4G, etc.) e se foi preciso TURN;
- se a imagem travou em algum momento, e quando;
- a curva da estimativa (as linhas `send:` a cada 2 s, ou pelo menos o início, o meio e o fim).

Idealmente rode duas vezes: uma entre duas redes "boas" e outra com uma ponta em rede móvel, que é onde o controle de congestionamento aparece.

## O que o ensaio ainda não cobre

- Reação a perda de pacotes forçada e a quedas de banda (exige limitar a rede; faça se souber).
- Simulcast (ver `M2-RELATORIO.md`).
- Encode e decode por hardware (M3).
