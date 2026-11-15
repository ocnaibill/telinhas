# Telinhas — Marcos do MVP (rascunho)

> Ordem de trabalho confirmada na DEC-53. Cada marco termina com um critério de saída verificável; nenhum marco começa sem o anterior fechado, exceto onde indicado.

Convenção: **RF** são os requisitos funcionais e **RNF** os não funcionais (metas provisórias, DEC-52).

## M0 — Scaffold

- Workspace Cargo com os crates `app` e `signaling` (crates de captura e codec só se a necessidade aparecer).
- CI em GitHub Actions: `fmt`, `clippy`, testes, em Linux, Windows e macOS (RNF-27).
- Arquivos de projeto: `.gitignore`, `rust-toolchain.toml`, diretrizes de contribuição curtas.

**Saída:** `cargo test` verde nos três sistemas no CI.

## M1 — Sinalização mínima

- Servidor com WebSocket, JSON e versão de protocolo (DEC-46).
- Criar sala, entrar por código, sair, estados ativa/vazia/expirada, expiração de 5 minutos (DEC-23).
- Token de sessão e reconexão (DEC-42, DEC-50), administrador e transferência (DEC-22, DEC-49).
- Limite de tentativas por IP (DEC-48).
- Cliente de linha de comando para testar, sem interface gráfica.

**Saída:** testes de integração cobrindo todo o fluxo de `docs/PROTOCOLO.md` §4 a §8. Cobre RF-01, RF-02, RF-13 (parte de rede) e RF-15.

## M2 — Vídeo P2P simples (decisão sobre o webrtc-rs) — concluído (DEC-57)

- Dois clientes na mesma sala trocam vídeo de uma fonte sintética (padrão de teste com contador de tempo), codificada em H.264 por software com `openh264` (DEC-56).
- Negociação por `signal` e STUN público; TURN opcional (DEC-21).
- Receptor **sem janela**: decodifica, valida os quadros e mede a latência pelo contador desenhado na imagem. A janela mínima passa para o M4 (DEC-40, DEC-56).
- **Teste da DEC-38:** verificar no `webrtc` 0.21.x controle de congestionamento, estabilidade, suporte a H.264 e a simulcast.

**Saída:** vídeo estável por 10 minutos entre duas máquinas em redes diferentes. Relatório curto: o `webrtc-rs` serve ou trocamos (`str0m` ou libwebrtc). Essa é uma decisão do mantenedor. O ensaio entre redes é feito pelo mantenedor, com o guia em `docs/ENSAIO-M2.md`.

## M3 — Captura real e encode por hardware (uma plataforma)

- Captura de monitor e de janela no Windows (Windows Graphics Capture; DEC-53).
- Encode H.264 por hardware via FFmpeg atrás da trait `Encoder` (DEC-39), fallback por software com aviso (RF-14).
- Escolha de resolução e FPS vinda do hardware e da fonte (RF-05, RF-22).
- **Medição:** bitrates reais de 1080p60 para substituir a suposição de ~8 Mbps por espectador.

**Saída:** transmissão de tela real entre duas máquinas dentro de RNF-01, RNF-03 e RNF-04, com os números medidos registrados no repositório. Cobre RF-03 e RF-05.

## M4 — Sala completa e interface principal

- Interface principal única (DEC-20), escolhida a biblioteca de UI com as telas desenhadas (DEC-40).
- Início, apelido no primeiro uso, sala com Grid dinâmico e Foco, modal de configuração de transmissão.
- Administrador, aprovação de entrada, estado "reconectando", monitor de latência.
- Volume local por transmissão, copiar código da sala.

**Saída:** duas ou mais pessoas usam o app inteiro, sem linha de comando. Cobre RF-01, RF-02, RF-11, RF-12, RF-13, RF-15, RF-16, RF-17, RF-18, RF-19, RF-20 e RF-21.

## M5 — Áudio

- Áudio de janela específica, do sistema e do microfone (DEC-16).
- Exclusão de aplicativos do áudio ao transmitir tela inteira, validada em protótipo (DEC-41).

**Saída:** transmitir tela inteira sem que a voz do Discord volte a quem fala. Cobre RF-08.

## M6 — Camadas e multi-transmissão

- Camada baixa sob demanda e escolha "original / econômica" (DEC-04, DEC-05; RF-07).
- Várias transmissões simultâneas na mesma sala (DEC-06).
- Troca de janela sem derrubar (DEC-11, DEC-44; RF-10).
- Webcam em PiP separado (DEC-12; RF-04), captura de mouse (DEC-17; RF-09).
- Estimativa de banda e aviso acima de 12 (DEC-24; RF-06).
- Adaptação de bitrate por espectador.

**Saída:** sala de 4 a 6 pessoas, com duas transmissões simultâneas e camada baixa ligando e desligando conforme a demanda.

## M7 — Demais plataformas e validação com 12

- Portar captura, áudio e encode para as outras duas plataformas (macOS 13+, Linux/Wayland; DEC-28, DEC-29, DEC-45).
- Ensaio com 12 pessoas e medição das metas de RNF.
- Instaladores por sistema (RNF-26).

**Saída:** versão 0.1 publicada, com os números medidos de cada RNF registrados e as metas revisadas.

## Pontos ainda sem decisão

- **Design das telas** (itens que faltam no mockup) deve estar pronto antes do M4. Pode ser feito em paralelo ao M1 a M3.
- **Hospedagem do servidor** (homelab, DEC-21): configuração do tunnel e do TURN entra junto do M2.
