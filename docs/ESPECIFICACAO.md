# Telinhas — Especificação

> Status: especificação, antes do código (15/11/2026). Decisões do mantenedor viram DEC-nnn e não são reabertas sem pedido dele.

## 1. Visão

Telinhas é um app desktop (macOS, Windows, Linux) para **transmitir tela, janela e, opcionalmente, webcam** para outras pessoas, fora de uma ferramenta de call.

Caso de uso central: o grupo já está em call (ex.: Discord), mas o compartilhamento de tela da ferramenta está indisponível ou ruim. O Telinhas faz só a transmissão, em paralelo à call.

**Não é:** app de call, chat, ou "tudo em um lugar".

## 2. Decisões

| ID | Decisão | Origem |
|----|---------|--------|
| DEC-01 | Implementado em Rust, com foco em velocidade e baixo uso de memória. Interface nativa em Rust (não Tauri/webview). | Mantenedor |
| DEC-02 | Vídeo trafega P2P (WebRTC). Existe um servidor mínimo de sinalização. STUN e TURN de apoio. Detalhes em ARQUITETURA.md. | Mantenedor + recomendação |
| DEC-03 | Escopo dimensionado para **12 pessoas simultâneas** por sala, sem limite rígido. O app informa a banda necessária ao usuário. | Mantenedor |
| DEC-04 | Quem transmite escolhe a qualidade máxima (resolução e FPS), limitada pelo hardware. O espectador escolhe entre "original" e uma camada mais baixa. | Mantenedor |
| DEC-05 | Camadas baixas são criadas **sob demanda**: só são codificadas se algum espectador pedir. | Mantenedor |
| DEC-06 | **Qualquer participante pode transmitir**, inclusive várias pessoas ao mesmo tempo na mesma sala. | Mantenedor |
| DEC-07 | Salas são efêmeras. Código de **8 caracteres**, alfabeto de 32 símbolos sem ambiguidade (≈ 2⁴⁰ combinações), mostrado como `XXXX-XXXX`. Quem tem o código entra. | Mantenedor + recomendação |
| DEC-08 | Aprovação de entrada é **opcional**, desligada por padrão. Quem administra a sala pode exigi-la. | Mantenedor |
| DEC-09 | A sala **não morre** quando o criador sai. Ela vive enquanto houver pessoas nela. A figura de "host" é só a de administrador, transferível (ver §4). | Mantenedor |
| DEC-10 | Espectador que cai pode reconectar usando o mesmo código. O código identifica a sala, não o usuário. | Mantenedor |
| DEC-11 | Dá para trocar a janela/tela compartilhada **sem derrubar** a transmissão nem a sala. | Mantenedor |
| DEC-12 | Webcam é um vídeo separado, exibido em picture-in-picture. | Mantenedor |
| DEC-13 | Chat de texto está fora do escopo. | Mantenedor |
| DEC-14 | No primeiro uso a pessoa cria um apelido, que pode ser trocado a qualquer momento. Não há conta. | Mantenedor |
| DEC-15 | Codec padrão H.264 com aceleração por hardware. Há fallback por software, com **aviso** de que a transmissão pode ficar quebrada. | Mantenedor |
| DEC-16 | Áudio: se uma janela específica é capturada, só o áudio dela é transmitido. Se a tela inteira é capturada, o usuário pode **excluir aplicativos** do áudio do sistema (ex.: Discord, para evitar eco). O microfone é fonte opcional e fica desligado por padrão. | Mantenedor |
| DEC-17 | Mouse: o usuário escolhe se captura. Padrão desligado ao capturar janela específica e ligado ao capturar monitor inteiro. Exceção: janelas de navegadores conhecidos têm o padrão ligado. | Mantenedor |
| DEC-18 | Conteúdo protegido por DRM aparece como tela preta. Não há contorno. | Mantenedor |
| DEC-19 | A interface exibe um monitor com a **latência (ms)** de cada participante naquela sala. | Mantenedor |
| DEC-20 | Uma única tela principal, que muda de tamanho e exibe elementos como pop-ups/componentes. | Mantenedor |
| DEC-21 | Projeto open-source. Sinalização (e TURN) hospedados no homelab do mantenedor, atrás do Cloudflare Tunnel, com as portas necessárias abertas. | Mantenedor |
| DEC-22 | Administrador da sala: o criador no início. Ao sair, o papel passa ao participante mais antigo na sala. Só o administrador aprova entradas e liga/desliga a exigência de aprovação. | Mantenedor (aceitou proposta) |
| DEC-23 | Sala vazia mantém o código válido por 5 minutos para reconexão; depois expira. | Mantenedor (aceitou proposta) |
| DEC-24 | O limite de 12 é só dimensionamento: o app avisa sobre banda e desempenho a partir daí, sem bloquear entradas. | Mantenedor (aceitou proposta) |
| DEC-25 | Código de sala expirado pode ser gerado de novo, aleatoriamente, para outra sala. | Mantenedor (aceitou proposta) |
| DEC-26 | Apelidos repetidos na mesma sala são permitidos; o app acrescenta um sufixo (ex.: `Bianco (2)`). | Mantenedor (aceitou proposta) |
| DEC-27 | Navegadores com mouse ligado por padrão: lista inicial Chrome, Firefox, Edge, Safari, Brave, Arc, Opera e Vivaldi, em arquivo de configuração editável. | Mantenedor (aceitou proposta) |
| DEC-28 | Mínimo de macOS 13 (captura de áudio por aplicativo via ScreenCaptureKit). | Mantenedor (aceitou proposta) |
| DEC-29 | No Linux/Wayland, a seleção de tela/janela usa a UI do portal do sistema (`xdg-desktop-portal`). | Mantenedor (aceitou proposta) |
| DEC-30 | O mockup do Stitch é só ponto de partida visual (paleta, componentes, composição). Quem define as regras é esta especificação, nunca o mockup. | Mantenedor |
| DEC-31 | O nome é **Telinhas**. Do mockup saem: chat, nome de exemplo, estatísticas decorativas, textos e rodapé de página web, menu de navegação entre páginas e promessas ("ilimitado", "sem limites de tempo", "até 4K"). Eram exemplos ou ideias futuras. | Mantenedor |
| DEC-32 | **Não há senha de sala.** A segurança é o código de 8 caracteres (DEC-07) mais a aprovação opcional do administrador (DEC-08, desligada por padrão). | Mantenedor |
| DEC-33 | Layouts de visualização na sala: **Grid dinâmico** e **Foco**. O layout 50/50 do mockup fica de fora. | Mantenedor |
| DEC-34 | Cada espectador tem volume local por transmissão, que só afeta o próprio app. | Mantenedor |
| DEC-35 | Ao entrar numa sala, a pessoa escolhe o modo: **Assistindo** (mic e tela desligados) ou **Transmitindo** (abre a configuração de transmissão logo ao entrar). | Mantenedor |
| DEC-36 | A configuração de transmissão é um **modal** sobre a sala (DEC-20). Resoluções e FPS oferecidos vêm do hardware e da fonte capturada (DEC-04), não de uma lista fixa. | Mantenedor |
| DEC-37 | Tudo o que o mockup não mostra e esta especificação exige entra no design: ver `docs/design/README.md`. | Mantenedor |
| DEC-38 | WebRTC: começar com `webrtc-rs`. Será testado na prática e, se não der conta (controle de congestionamento, simulcast, estabilidade), reavaliamos `str0m` ou binding de libwebrtc. | Mantenedor |
| DEC-39 | Codecs: FFmpeg (LGPL, ligação dinâmica) atrás de uma trait `Encoder`. APIs nativas por sistema podem substituir cada backend depois, se a medição pedir. | Mantenedor (aceitou proposta) |
| DEC-40 | Biblioteca de UI decidida junto com as telas. Critério principal: exibir vários vídeos decodificados como texturas de GPU. | Mantenedor (aceitou proposta) |
| DEC-41 | Captura com a API nativa de cada sistema (Windows Graphics Capture, ScreenCaptureKit, PipeWire). A exclusão de áudio por aplicativo será validada em protótipo nos três sistemas antes de ser tratada como garantida. | Mantenedor (aceitou proposta) |
| DEC-42 | Reconexão (DEC-10): o cliente gera um token de sessão, válido enquanto a sala existir. Com ele o servidor reconhece o participante e restaura papel de administrador, apelido e aprovação já concedida. | Mantenedor (aceitou proposta) |
| DEC-43 | O app avisa que, por ser P2P, os outros participantes da sala veem o IP do usuário. | Mantenedor (aceitou proposta) |
| DEC-44 | Trocar a fonte capturada (DEC-11) reconfigura o encoder para a nova resolução/proporção e força um keyframe, sem renegociar a conexão. | Mantenedor (aceitou proposta) |
| DEC-45 | No Linux o MVP suporta só Wayland (PipeWire + `xdg-desktop-portal`). X11 fica fora do escopo por enquanto. | Mantenedor (aceitou proposta) |

## 3. Requisitos funcionais (rascunho)

- **RF-01** Criar sala e receber o código de 8 caracteres.
- **RF-02** Entrar em sala existente digitando o código.
- **RF-03** Iniciar transmissão de monitor inteiro ou de uma janela.
- **RF-04** Iniciar/parar webcam independentemente da tela.
- **RF-05** Escolher resolução e FPS máximos, até o limite do hardware.
- **RF-06** Mostrar a banda de upload estimada, conforme qualidade e número de espectadores.
- **RF-07** Espectador escolher "original" ou "econômica".
- **RF-08** Selecionar aplicativos a excluir do áudio ao transmitir tela inteira.
- **RF-09** Ligar/desligar captura do mouse (padrões na DEC-17).
- **RF-10** Trocar a janela/tela transmitida sem encerrar a transmissão.
- **RF-11** Opção da sala de exigir aprovação para entrar.
- **RF-12** Mostrar lista de participantes (apelidos) e latência de cada um.
- **RF-13** Reconectar à sala após queda, com o mesmo código.
- **RF-14** Aviso ao usar encode por software.
- **RF-15** Definir e trocar o apelido (pedido no primeiro uso, DEC-14).
- **RF-16** Alternar a visualização da sala entre Grid dinâmico e Foco (DEC-33).
- **RF-17** Ajustar o volume local de cada transmissão (DEC-34).
- **RF-18** Escolher "Assistindo" ou "Transmitindo" ao entrar (DEC-35).
- **RF-19** Pop-up de pedido de entrada para o administrador, com aceitar/recusar (DEC-08, DEC-22).
- **RF-20** Indicar quem é o administrador da sala.
- **RF-21** Mostrar estado "reconectando" quando a conexão cai (DEC-10).
- **RF-22** Listar resoluções e FPS conforme o hardware e a fonte capturada (DEC-36).

## 4. Pontos em aberto

Nenhum por enquanto. Os pontos levantados em 15/11/2026 viraram DEC-22 a DEC-29, e os de tecnologia, reconexão e Linux viraram DEC-38 a DEC-45.

## 5. Fora do escopo

Chat, chamada de voz, controle remoto, gravação, contas de usuário, salas permanentes.
