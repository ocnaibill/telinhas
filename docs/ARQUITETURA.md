# Telinhas — Arquitetura (rascunho)

> Rascunho derivado das DECs em ESPECIFICACAO.md. Nada aqui é decisão do mantenedor até virar DEC.

## 1. Componentes

```
 ┌──────────┐   WebSocket (sinalização)   ┌────────────────────┐
 │ App      │ ◄─────────────────────────► │ Servidor de        │
 │ Telinhas │                             │ sinalização (Rust) │
 └────┬─────┘                             └────────────────────┘
      │  WebRTC (SRTP, criptografado)
      ▼
 ┌──────────┐        STUN / TURN (coturn, opcional, homelab)
 │ Outros   │
 │ apps     │
 └──────────┘
```

- **App (Rust):** captura, codificação, decodificação, renderização, UI.
- **Servidor de sinalização (Rust, stateless de mídia):** guarda só o estado efêmero das salas (membros, administrador, opções) e repassa SDP/ICE entre pares. Nenhum vídeo passa por ele. É isso que permite a sala continuar viva sem o criador (DEC-09).
- **STUN:** descoberta de IP/porta públicos. Serviços públicos servem para começar.
- **TURN (coturn):** relay de fallback para redes onde o P2P direto falha. Usa UDP, que o Cloudflare Tunnel não encaminha, então exige portas abertas no roteador (DEC-21).

## 2. Topologia de mídia

Malha (mesh) entre os participantes: cada pessoa que transmite envia o vídeo para cada um dos outros. Com 12 pessoas e uma transmissão, são 11 cópias saindo do transmissor (DEC-06).

Banda de upload estimada de quem transmite:

```
upload ≈ bitrate_camada_original × espectadores_na_original
       + bitrate_camada_baixa    × espectadores_na_baixa
```

Exemplo: 1080p60 a ~8 Mbps para 11 espectadores ≈ 88 Mbps. O app mostra essa estimativa antes e durante a transmissão (RF-06). Os bitrates reais serão medidos, não assumidos.

Camada baixa (DEC-05): codificada só quando existe ao menos um espectador pedindo. Quando o último sai dela, o encoder dela para.

## 3. Sala

- Estado vive no servidor de sinalização, em memória.
- Código: 8 símbolos de um alfabeto de 32 (Crockford base32 sem caracteres ambíguos), gerado com RNG criptográfico (DEC-07).
- Limite de tentativas de entrada por IP no servidor, para tornar inviável adivinhar códigos.
- Papel de administrador: criador no início; transfere-se ao participante mais antigo se ele sair (DEC-22).
- Sala sem membros expira após 5 minutos de graça (DEC-10, DEC-23). Depois disso o código pode ser sorteado de novo (DEC-25).
- Apelidos repetidos recebem sufixo, ex.: `Bianco (2)` (DEC-26).
- Acima de 12 participantes não há bloqueio, só aviso (DEC-24).

## 4. Captura por sistema

| Sistema | Vídeo | Áudio por aplicativo |
|---------|-------|----------------------|
| Windows | Windows Graphics Capture / DXGI | Process loopback (Windows 10 2004+), incluindo ou excluindo processo |
| macOS | ScreenCaptureKit | ScreenCaptureKit (macOS 13+); exige permissão de gravação de tela |
| Linux | PipeWire via `xdg-desktop-portal` | PipeWire, por nó de áudio |

Troca de janela sem derrubar (DEC-11): a fonte de captura é trocada por baixo da mesma trilha WebRTC, sem renegociar a conexão.

## 5. Codificação

- H.264 por hardware: NVENC, QSV, AMF, VideoToolbox, VAAPI (DEC-15).
- Fallback por software com aviso na UI.
- A decodificação no espectador também tenta aceleração por hardware.

## 6. Interface

- Rust nativo, sem webview (DEC-01). Candidatos: `egui` com `wgpu`, `iced`, `slint`. Decisão adiada até ver as telas do Stitch.
- Uma tela principal com componentes/pop-ups (DEC-20).

## 7. Latência

Medida por participante (RTT e atraso de exibição) e exibida em um monitor na tela (DEC-19).

## 8. Questões técnicas em aberto

- Biblioteca de WebRTC: `webrtc-rs` (Rust puro, menos maduro) ou binding de libwebrtc (mais maduro, dependência pesada).
- Biblioteca de codecs: FFmpeg ou APIs nativas por sistema.
- Como o app mede a banda de upload real do usuário para validar a estimativa.
- Estratégia de adaptação de bitrate (controle de congestionamento) por espectador.
