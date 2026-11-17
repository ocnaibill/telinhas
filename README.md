# Telinhas

App desktop (macOS, Windows e Linux) para **transmitir tela, janela e, opcionalmente, webcam** a outras pessoas, fora de uma ferramenta de call.

Caso de uso: o grupo está em call (por exemplo, no Discord), mas o compartilhamento de tela da ferramenta está indisponível ou ruim. O Telinhas faz só a transmissão, em paralelo à call.

**Não é** app de call, chat nem "tudo em um lugar".

## Como funciona

- Salas efêmeras, sem conta. Quem tem o código de 8 caracteres (`XXXX-XXXX`) entra; o administrador pode exigir aprovação.
- Qualquer participante pode transmitir, inclusive várias pessoas ao mesmo tempo.
- Vídeo vai direto entre os participantes (P2P, WebRTC). Um servidor mínimo de sinalização só guarda o estado das salas e repassa a negociação.
- H.264 com aceleração por hardware; o espectador escolhe entre qualidade original e econômica.
- Áudio por janela ou por tela, com exclusão de aplicativos (por exemplo, o Discord, para evitar eco).
- Escrito em Rust, com interface nativa.

Como o vídeo é P2P em malha, a banda de **upload** de quem transmite cresce com o número de espectadores. O app mostra a estimativa. O projeto é dimensionado para até 12 pessoas por sala.

## Estado do projeto

**Fase de especificação.** Ainda não há código; só a documentação em [`docs/`](docs/).

| Documento | Conteúdo |
|-----------|----------|
| [ESPECIFICACAO.md](docs/ESPECIFICACAO.md) | Fonte da verdade: decisões (DEC-nnn) e requisitos funcionais |
| [ARQUITETURA.md](docs/ARQUITETURA.md) | Rascunho técnico: componentes, topologia, captura e codificação |
| [PROTOCOLO.md](docs/PROTOCOLO.md) | Rascunho do protocolo de sinalização |
| [REQUISITOS-NAO-FUNCIONAIS.md](docs/REQUISITOS-NAO-FUNCIONAIS.md) | Metas provisórias de latência, recursos, rede e segurança |
| [MARCOS.md](docs/MARCOS.md) | Marcos do MVP (M0 a M7) e critérios de saída |
| [ENSAIO-M2.md](docs/ENSAIO-M2.md) | Guia do ensaio do M2 entre redes |
| [M2-RELATORIO.md](docs/M2-RELATORIO.md) | Relatório parcial do M2 sobre o `webrtc-rs` |
| [PLANO-M3.md](docs/PLANO-M3.md) | Plano do M3: captura e encode por hardware no Windows |
| [UI-BIBLIOTECA.md](docs/UI-BIBLIOTECA.md) | Escolha da biblioteca de UI (egui) e o que a interface precisa |
| [design/README.md](docs/design/README.md) | Mapa das telas e o que falta desenhar |

## Plataformas previstas

- Windows 10 (2004 ou mais novo)
- macOS 13 ou mais novo
- Linux com Wayland, PipeWire e `xdg-desktop-portal` (X11 fora do escopo por enquanto)

## Contribuir

O projeto é open-source e ainda está sendo definido. Decisões de produto seguem a especificação; abra uma issue antes de propor mudanças que alterem uma DEC.

## Licença

[Apache-2.0](LICENSE)
